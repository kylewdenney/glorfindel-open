use async_trait::async_trait;
use dust_dds::{
    domain::domain_participant_factory::DomainParticipantFactory,
    infrastructure::{
        qos::{DataReaderQos, DataWriterQos, QosKind},
        qos_policy::{
            DurabilityQosPolicy, DurabilityQosPolicyKind, ReliabilityQosPolicy,
            ReliabilityQosPolicyKind,
        },
        status::NO_STATUS,
        time::DurationKind,
    },
    subscription::sample_info::{ANY_INSTANCE_STATE, ANY_SAMPLE_STATE, ANY_VIEW_STATE},
    topic_definition::type_support::DdsType,
};
use glorfindel_schemas::{
    AgentResponse, CapabilityManifest, MessageEnvelope, TaskRequest,
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, error, info};

use crate::error::TransportError;
use crate::traits::ControlPlane;

/// DDS topic names following OMS naming conventions.
const TOPIC_TASKS_REQUEST: &str = "glorfindel/tasks/request";
const TOPIC_TASKS_RESPONSE: &str = "glorfindel/tasks/response";
const TOPIC_AGENTS_CAPABILITY: &str = "glorfindel/agents/capability";

/// DDS-backed control plane implementation using dust_dds.
///
/// Uses DDS topics with reliable QoS for task routing and agent discovery.
/// The DDS participant is created on the specified domain ID.
pub struct DdsControlPlane {
    domain_id: i32,
}

// DDS wrapper type for serialized message envelopes.
// dust_dds requires types to implement DdsType, so we wrap our JSON-serialized
// envelopes in a simple container.
// payload is Vec<u8> (CDR octet sequence) rather than String — CDR bounded strings
// default to 256 bytes in dust_dds, which is far too small for intent payloads.
// Octet sequences are unbounded by default.
#[derive(Debug, Clone, Serialize, Deserialize, DdsType)]
struct DdsMessage {
    topic: String,
    payload: Vec<u8>,
}

impl DdsControlPlane {
    pub fn new(domain_id: i32) -> Self {
        info!(domain_id, "Creating DDS control plane");
        Self { domain_id }
    }

    fn serialize_envelope<T: Serialize>(envelope: &MessageEnvelope<T>) -> Result<Vec<u8>, TransportError> {
        serde_json::to_vec(envelope).map_err(|e| TransportError::Serialization(e.to_string()))
    }

    fn deserialize_envelope<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<MessageEnvelope<T>, TransportError> {
        serde_json::from_slice(bytes).map_err(|e| TransportError::Serialization(e.to_string()))
    }

    fn reliable_writer_qos() -> DataWriterQos {
        let mut qos = DataWriterQos::default();
        qos.reliability = ReliabilityQosPolicy {
            kind: ReliabilityQosPolicyKind::Reliable,
            max_blocking_time: DurationKind::Finite(std::time::Duration::from_secs(1).into()),
        };
        qos.durability = DurabilityQosPolicy {
            kind: DurabilityQosPolicyKind::TransientLocal,
        };
        qos
    }

    fn reliable_reader_qos() -> DataReaderQos {
        let mut qos = DataReaderQos::default();
        qos.reliability = ReliabilityQosPolicy {
            kind: ReliabilityQosPolicyKind::Reliable,
            max_blocking_time: DurationKind::Finite(std::time::Duration::from_secs(1).into()),
        };
        qos.durability = DurabilityQosPolicy {
            kind: DurabilityQosPolicyKind::TransientLocal,
        };
        qos
    }
}

#[async_trait]
impl ControlPlane for DdsControlPlane {
    async fn publish_task(
        &self,
        task: MessageEnvelope<TaskRequest>,
    ) -> Result<(), TransportError> {
        let json = Self::serialize_envelope(&task)?;
        let msg = DdsMessage {
            topic: TOPIC_TASKS_REQUEST.into(),
            payload: json,
        };

        let participant = DomainParticipantFactory::get_instance()
            .create_participant(self.domain_id, QosKind::Default, None, NO_STATUS)
            .map_err(|e| TransportError::Dds(format!("{e:?}")))?;

        let topic = participant
            .create_topic::<DdsMessage>(TOPIC_TASKS_REQUEST, "DdsMessage", QosKind::Default, None, NO_STATUS)
            .map_err(|e| TransportError::Dds(format!("{e:?}")))?;

        let publisher = participant
            .create_publisher(QosKind::Default, None, NO_STATUS)
            .map_err(|e| TransportError::Dds(format!("{e:?}")))?;

        let writer = publisher
            .create_datawriter::<DdsMessage>(&topic, QosKind::Specific(Self::reliable_writer_qos()), None, NO_STATUS)
            .map_err(|e| TransportError::Dds(format!("{e:?}")))?;

        writer
            .write(&msg, None)
            .map_err(|e| TransportError::Dds(format!("{e:?}")))?;

        debug!(task_id = %task.payload.task_id, "Published task request via DDS");
        Ok(())
    }

    async fn subscribe_tasks(
        &self,
    ) -> Result<mpsc::Receiver<MessageEnvelope<TaskRequest>>, TransportError> {
        let (tx, rx) = mpsc::channel(256);
        let domain_id = self.domain_id;

        tokio::spawn(async move {
            let participant = match DomainParticipantFactory::get_instance()
                .create_participant(domain_id, QosKind::Default, None, NO_STATUS)
            {
                Ok(p) => p,
                Err(e) => {
                    error!("Failed to create DDS participant: {e:?}");
                    return;
                }
            };

            let topic = match participant.create_topic::<DdsMessage>(
                TOPIC_TASKS_REQUEST,
                "DdsMessage",
                QosKind::Default,
                None,
                NO_STATUS,
            ) {
                Ok(t) => t,
                Err(e) => {
                    error!("Failed to create DDS topic: {e:?}");
                    return;
                }
            };

            let subscriber = match participant.create_subscriber(QosKind::Default, None, NO_STATUS) {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to create DDS subscriber: {e:?}");
                    return;
                }
            };

            let reader = match subscriber.create_datareader::<DdsMessage>(
                &topic,
                QosKind::Specific(DdsControlPlane::reliable_reader_qos()),
                None,
                NO_STATUS,
            ) {
                Ok(r) => r,
                Err(e) => {
                    error!("Failed to create DDS reader: {e:?}");
                    return;
                }
            };

            loop {
                match reader.take(32, ANY_SAMPLE_STATE, ANY_VIEW_STATE, ANY_INSTANCE_STATE) {
                    Ok(samples) => {
                        for sample in samples {
                            if let Ok(data) = sample.data() {
                                match DdsControlPlane::deserialize_envelope::<TaskRequest>(
                                    &data.payload,
                                ) {
                                    Ok(envelope) => {
                                        if tx.send(envelope).await.is_err() {
                                            return;
                                        }
                                    }
                                    Err(e) => {
                                        error!("Failed to deserialize task request: {e}");
                                    }
                                }
                            }
                        }
                    }
                    Err(_) => {}
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        });

        Ok(rx)
    }

    async fn publish_capability(
        &self,
        manifest: MessageEnvelope<CapabilityManifest>,
    ) -> Result<(), TransportError> {
        let json = Self::serialize_envelope(&manifest)?;
        let msg = DdsMessage {
            topic: TOPIC_AGENTS_CAPABILITY.into(),
            payload: json,
        };

        let participant = DomainParticipantFactory::get_instance()
            .create_participant(self.domain_id, QosKind::Default, None, NO_STATUS)
            .map_err(|e| TransportError::Dds(format!("{e:?}")))?;

        let topic = participant
            .create_topic::<DdsMessage>(
                TOPIC_AGENTS_CAPABILITY,
                "DdsMessage",
                QosKind::Default,
                None,
                NO_STATUS,
            )
            .map_err(|e| TransportError::Dds(format!("{e:?}")))?;

        let publisher = participant
            .create_publisher(QosKind::Default, None, NO_STATUS)
            .map_err(|e| TransportError::Dds(format!("{e:?}")))?;

        let writer = publisher
            .create_datawriter::<DdsMessage>(
                &topic,
                QosKind::Specific(Self::reliable_writer_qos()),
                None,
                NO_STATUS,
            )
            .map_err(|e| TransportError::Dds(format!("{e:?}")))?;

        writer
            .write(&msg, None)
            .map_err(|e| TransportError::Dds(format!("{e:?}")))?;

        info!(agent_id = %manifest.payload.agent_id, "Published capability manifest via DDS");
        Ok(())
    }

    async fn subscribe_capabilities(
        &self,
    ) -> Result<mpsc::Receiver<MessageEnvelope<CapabilityManifest>>, TransportError> {
        let (tx, rx) = mpsc::channel(64);
        let domain_id = self.domain_id;

        tokio::spawn(async move {
            let participant = match DomainParticipantFactory::get_instance()
                .create_participant(domain_id, QosKind::Default, None, NO_STATUS)
            {
                Ok(p) => p,
                Err(e) => {
                    error!("Failed to create DDS participant: {e:?}");
                    return;
                }
            };

            let topic = match participant.create_topic::<DdsMessage>(
                TOPIC_AGENTS_CAPABILITY,
                "DdsMessage",
                QosKind::Default,
                None,
                NO_STATUS,
            ) {
                Ok(t) => t,
                Err(e) => {
                    error!("Failed to create DDS topic: {e:?}");
                    return;
                }
            };

            let subscriber = match participant.create_subscriber(QosKind::Default, None, NO_STATUS) {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to create DDS subscriber: {e:?}");
                    return;
                }
            };

            let reader = match subscriber.create_datareader::<DdsMessage>(
                &topic,
                QosKind::Specific(DdsControlPlane::reliable_reader_qos()),
                None,
                NO_STATUS,
            ) {
                Ok(r) => r,
                Err(e) => {
                    error!("Failed to create DDS reader: {e:?}");
                    return;
                }
            };

            loop {
                match reader.take(32, ANY_SAMPLE_STATE, ANY_VIEW_STATE, ANY_INSTANCE_STATE) {
                    Ok(samples) => {
                        for sample in samples {
                            if let Ok(data) = sample.data() {
                                match DdsControlPlane::deserialize_envelope::<CapabilityManifest>(
                                    &data.payload,
                                ) {
                                    Ok(envelope) => {
                                        if tx.send(envelope).await.is_err() {
                                            return;
                                        }
                                    }
                                    Err(e) => {
                                        error!("Failed to deserialize capability: {e}");
                                    }
                                }
                            }
                        }
                    }
                    Err(_) => {}
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        });

        Ok(rx)
    }

    async fn publish_response(
        &self,
        response: MessageEnvelope<AgentResponse>,
    ) -> Result<(), TransportError> {
        let json = Self::serialize_envelope(&response)?;
        let msg = DdsMessage {
            topic: TOPIC_TASKS_RESPONSE.into(),
            payload: json,
        };

        let participant = DomainParticipantFactory::get_instance()
            .create_participant(self.domain_id, QosKind::Default, None, NO_STATUS)
            .map_err(|e| TransportError::Dds(format!("{e:?}")))?;

        let topic = participant
            .create_topic::<DdsMessage>(TOPIC_TASKS_RESPONSE, "DdsMessage", QosKind::Default, None, NO_STATUS)
            .map_err(|e| TransportError::Dds(format!("{e:?}")))?;

        let publisher = participant
            .create_publisher(QosKind::Default, None, NO_STATUS)
            .map_err(|e| TransportError::Dds(format!("{e:?}")))?;

        let writer = publisher
            .create_datawriter::<DdsMessage>(
                &topic,
                QosKind::Specific(Self::reliable_writer_qos()),
                None,
                NO_STATUS,
            )
            .map_err(|e| TransportError::Dds(format!("{e:?}")))?;

        writer
            .write(&msg, None)
            .map_err(|e| TransportError::Dds(format!("{e:?}")))?;

        debug!(task_id = %response.payload.task_id, "Published agent response via DDS");
        Ok(())
    }

    async fn subscribe_responses(
        &self,
    ) -> Result<mpsc::Receiver<MessageEnvelope<AgentResponse>>, TransportError> {
        let (tx, rx) = mpsc::channel(256);
        let domain_id = self.domain_id;

        tokio::spawn(async move {
            let participant = match DomainParticipantFactory::get_instance()
                .create_participant(domain_id, QosKind::Default, None, NO_STATUS)
            {
                Ok(p) => p,
                Err(e) => {
                    error!("Failed to create DDS participant: {e:?}");
                    return;
                }
            };

            let topic = match participant.create_topic::<DdsMessage>(
                TOPIC_TASKS_RESPONSE,
                "DdsMessage",
                QosKind::Default,
                None,
                NO_STATUS,
            ) {
                Ok(t) => t,
                Err(e) => {
                    error!("Failed to create DDS topic: {e:?}");
                    return;
                }
            };

            let subscriber = match participant.create_subscriber(QosKind::Default, None, NO_STATUS) {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to create DDS subscriber: {e:?}");
                    return;
                }
            };

            let reader = match subscriber.create_datareader::<DdsMessage>(
                &topic,
                QosKind::Specific(DdsControlPlane::reliable_reader_qos()),
                None,
                NO_STATUS,
            ) {
                Ok(r) => r,
                Err(e) => {
                    error!("Failed to create DDS reader: {e:?}");
                    return;
                }
            };

            loop {
                match reader.take(32, ANY_SAMPLE_STATE, ANY_VIEW_STATE, ANY_INSTANCE_STATE) {
                    Ok(samples) => {
                        for sample in samples {
                            if let Ok(data) = sample.data() {
                                match DdsControlPlane::deserialize_envelope::<AgentResponse>(
                                    &data.payload,
                                ) {
                                    Ok(envelope) => {
                                        if tx.send(envelope).await.is_err() {
                                            return;
                                        }
                                    }
                                    Err(e) => {
                                        error!("Failed to deserialize response: {e}");
                                    }
                                }
                            }
                        }
                    }
                    Err(_) => {}
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        });

        Ok(rx)
    }
}

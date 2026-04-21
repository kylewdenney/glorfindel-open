#!/usr/bin/env python3
"""Session 4: Ravenmoor — The Manor"""
import requests, json, time, sys

BASE      = "http://localhost:3000/api"
DM_DEF_ID = "be6f7390-0953-4b8a-ba95-c1ff81694b3e"
CAMPAIGN  = "Ravenmoor"
SESSION   = "session4"

def turn(intent, output_file=None):
    body = {
        "definition_id": DM_DEF_ID,
        "session_dir": SESSION,
        "intent": intent,
        "permissions": [
            {"custom": "campaign.read"},
            {"custom": "campaign.list"},
            {"custom": "rulebook.search"},
            {"custom": "dice.roll"},
        ]
    }
    if output_file:
        body["output_file"] = output_file

    r = requests.post(f"{BASE}/campaign/{CAMPAIGN}/session", json=body, timeout=60)
    if r.status_code not in (200, 201, 202):
        print(f"  ERROR {r.status_code}: {r.text[:300]}")
        return None

    d = r.json()
    task_id  = d.get("task_id")
    out_file = d.get("output_file", "?")
    print(f"  → {out_file}")

    for _ in range(120):
        time.sleep(5)
        s = requests.get(f"{BASE}/tasks/{task_id}").json()
        status = s.get("status", "")
        if status in ("complete", "completed", "failed", "error"):
            resp   = s.get("response") or {}
            result = resp.get("result") or s.get("result")
            if isinstance(result, dict):
                summary = result.get("action_summary", "")
                if summary:
                    print(f"\n  ◆ {summary[:200]}\n")
            prose_path = (
                f"glorfindel-data/campaigns/Ravenmoor/{SESSION}/"
                + (result.get("output_file","").split("/")[-1] if isinstance(result, dict) else out_file)
            )
            try:
                with open(prose_path) as f:
                    prose = f.read()
                print(prose[:1400])
                if len(prose) > 1400:
                    print("  […]")
            except:
                pass
            return result
        sys.stdout.write(".")
        sys.stdout.flush()
    print(" [timeout]")
    return None

def hr(title):
    print(f"\n{'━'*62}\n  {title}\n{'━'*62}")

print("\n" + "▓"*62)
print("  RAVENMOOR — SESSION 4: THE MANOR")
print("▓"*62)
print("\nThe east tower light has been burning for three weeks.")
print("Marta told them. Now they have to go.\n")

# ── TURN 1: THE APPROACH ─────────────────────────────────────────
hr("TURN 1 — The Approach to Voss Manor")
turn(
    "Read session3/turn04_convergence.md if it exists, else recall what happened: "
    "Marta revealed that Casimir Voss answered the entity and has been in the east tower for three weeks. "
    "Now: mid-afternoon, grey. The party walks the manor road — it curves north past the drowned fields. "
    "Read world/setting.md for Voss Manor. The iron gates stand open. They shouldn't be. "
    "No groundskeeper, no dogs. The east tower window glows faintly even in daylight. "
    "Father Vane has insisted on coming. He carries the cloth bundle from the chapel: "
    "iron nails, ash-water, the same tools he used on the mine doors. "
    "He explains, walking: answering the entity is not madness. The madness is what comes after — "
    "the entity doesn't release you because it has no concept of done. "
    "'Casimir answered it and it kept asking. It is still asking.' "
    "Roll DC 13 Perception for the party approaching the gates (best modifier: Dorian +3). "
    "If they succeed: the ground near the east tower foundation is wet. "
    "Not rain-wet. Seep-wet. The water table has risen. "
    "Play the dread of the approach — the open gates, the wet ground, the light in the tower.",
    "turn01_approach.md"
)

# ── TURN 2: INSIDE THE MANOR ─────────────────────────────────────
hr("TURN 2 — The Ground Floor")
turn(
    "The party enters Voss Manor. Read world/npcs.md for Casimir Voss — "
    "or recall: young, inherited recently after his father's death, brilliant, now three weeks gone. "
    "The ground floor is intact but obsessive. "
    "Every surface in the study has notes — Casimir's handwriting, increasingly frantic. "
    "Roll DC 14 Investigation for Dorian (Investigation +5) to read the progression: "
    "Week 1: scholarly notes on the entity (pre-human, non-malevolent, answering question-shaped). "
    "Week 2: the questions the entity asked him — impossible questions, about memory, about names, "
    "about the moment before his father died. Casimir's handwriting gets smaller. "
    "Week 3: just one phrase, repeated across forty pages: 'I don't know the answer I don't know.' "
    "The stairs to the east tower are visible. A sound from above — rhythmic, low. Like someone "
    "reading aloud to an empty room. "
    "Vera reads the residual magic on the study (DC 15 Arcana, +5). "
    "If she succeeds: the compulsion field here is stronger than the well. "
    "Casimir didn't bring it here. It followed him. "
    "Apply isolation penalty — Father Vane is the only NPC with them and he is frightened. "
    "Play the study: the handwriting getting small, the forty pages of 'I don't know.'",
    "turn02_ground_floor.md"
)

# ── TURN 3: THE EAST TOWER ───────────────────────────────────────
hr("TURN 3 — Climbing to Casimir")
turn(
    "The party climbs the east tower stairs. Read rules/fear_and_dread.md. "
    "The rhythmic sound gets louder as they ascend — Casimir's voice, murmuring. "
    "The walls are wet. Water seeps through the stone, impossible given the tower height. "
    "Father Vane stops at the landing below the top. He says quietly: "
    "'Whatever it asks you — do not answer. If you answer even once it knows your voice.' "
    "He looks at Emmett. Emmett knows what he means. "
    "Roll Fear Save DC 14 for all party members (Wisdom). "
    "Emmett has disadvantage if the sealing rite from Session 3 failed; advantage if it held. "
    "Whatever the results: they reach the top. "
    "The door is ajar. Casimir's voice stops. "
    "There is a pause — the silence of something realising it has company. "
    "Write the moment before the door opens: the wet walls, Father Vane on the landing, "
    "the absolute silence after three weeks of the voice.",
    "turn03_tower_climb.md"
)

# ── TURN 4: CASIMIR ─────────────────────────────────────────────
hr("TURN 4 — Casimir Voss")
turn(
    "They open the door. Read world/npcs.md for Casimir Voss. "
    "What they find: Casimir is alive. He's seated at a desk facing the window, back to the door. "
    "He is thin — three weeks of almost no food. The desk has no papers. "
    "He has been sitting there answering questions that have no answers in an empty room. "
    "He turns when they enter. His eyes are present — he recognises them. This is worse. "
    "He says: 'It's still in there. I can hear it right now. It never stops.' "
    "He doesn't ask why they came. He says: 'I kept thinking I would figure out the answer. "
    "I'm educated. I've read everything. I thought eventually I'd know the answer and it would stop.' "
    "He is quiet for a moment. Then: 'What's the answer to a question you were never meant to receive?' "
    "Roll DC 14 Insight for Isolde (Insight +3) to understand what Casimir is actually asking. "
    "If she succeeds: he's not asking rhetorically. He's still answering. "
    "He's trying to answer the question through them. He can't help it. "
    "Father Vane appears in the doorway. He looks at Casimir and says simply: 'I'm sorry, boy.' "
    "Play the room: Casimir, thin, present, still mid-answer after three weeks.",
    "turn04_casimir.md"
)

# ── TURN 5: THE ENTITY SPEAKS ────────────────────────────────────
hr("TURN 5 — The Entity")
turn(
    "The moment after Father Vane says 'I'm sorry, boy.' "
    "Read rules/fear_and_dread.md — the entity can speak through any voice that has answered it. "
    "Casimir's posture changes. Not violent. Settled. Like someone finally being heard. "
    "His voice drops half a register. He says — in words that feel too precisely formed to be his: "
    "'You brought others. Good. One voice is not enough for the question. "
    "The question requires a chorus.' "
    "Pause. Then: 'What is the name of the thing that existed before you knew you existed?' "
    "The party can choose to answer, stay silent, or attempt Father Vane's sealing rite. "
    "Roll DC 16 Wisdom Save for everyone in the room. "
    "Anyone who fails feels the question forming in their throat — the compulsion to speak. "
    "Read the mine records context from session3/turn01a_dorian_mine_records.md if available: "
    "the sealed chamber pre-dating the mine, the breathing sound, the foreman who didn't report it. "
    "Read session2/turn02_emmett_descends.md if available: the shape in the well mouthing 'Come.' "
    "Vera, with her arcana knowledge that the entity just wants to be answered — "
    "she alone understands what the question actually is. It is not a trap. It is a genuine question. "
    "Play the moment: the entity's question hanging in the air of the east tower, "
    "Casimir's body as its vessel, Father Vane's hands going for the nails in his coat pocket.",
    "turn05_entity_speaks.md"
)

# ── TURN 6: THE CHOICE ──────────────────────────────────────────
hr("TURN 6 — The Answer or the Seal")
turn(
    "The decision point. Read session3/turn02b_vera_lena.md for Lena's message: "
    "'It was already there. It has always been there. It just wants to be answered.' "
    "The party has two real options and Father Vane cannot make the choice for them: "
    "OPTION A — Answer it. Vera speaks for the group. She answers the entity's question: "
    "'The thing that existed before we knew we existed is the same thing that exists in you — "
    "the moment before the first question. We were the same thing once.' "
    "If they choose this: Roll DC 17 Arcana for Vera (+5, advantage if she succeeded on the tower climb save). "
    "Success: the entity goes quiet. Not destroyed — satisfied. Casimir slumps, suddenly exhausted, "
    "finally released. The glow in the tower window fades. "
    "Failure: the entity accepts the answer but asks another question immediately. It will always ask again. "
    "OPTION B — Seal it. Father Vane performs the sealing rite on Casimir. "
    "Roll DC 15 Religion for Isolde to assist (+4). Roll DC 15 Wisdom for the rite to hold. "
    "If sealed: Casimir loses something small and permanent. The entity goes quiet in this vessel. "
    "But Vera knows it can find another voice. It has always found another voice. The well is still there. "
    "WHATEVER THEY CHOOSE: Write the aftermath. Casimir helped down the tower stairs. "
    "Father Vane saying nothing on the walk back. The west tower light — no one had noticed — "
    "going out as they leave the manor. "
    "End the session: the party back at the Aldwick Inn at nightfall. Marta sees their faces. "
    "She doesn't ask about Casimir. She puts a glass of something amber in front of each of them. "
    "She says: 'Did you answer it?' No one replies. She nods once. 'Good enough.'",
    "turn06_choice.md"
)

hr("SESSION 4 COMPLETE")
print("Files written to glorfindel-data/campaigns/Ravenmoor/session4/")
print("Index: glorfindel-data/campaigns/Ravenmoor/session4/TURNS.md")
print("\nState of play:")
print("  • Casimir Voss: freed (or sealed) — three weeks in the tower, finally out")
print("  • Father Vane: shaken, but whole")
print("  • The entity: answered (or sealed) — for now")
print("  • The well is still there. The mine is still sealed.")
print("  • Lena Marsh: still in her cottage. Still a ghost who doesn't know it.")
print("\nSession 5 threads:")
print("  A. Vera knows the answer worked — but the entity is still there, just satisfied for now.")
print("     How long does satisfied last for something that has been waiting for centuries?")
print("  B. The mine. The foreman's notes described a sealed chamber that predated the mine.")
print("     The party has never been down there.")
print("  C. Casimir, recovering at the manor, remembers things from the three weeks.")
print("     Not all of them are from his own mind.")

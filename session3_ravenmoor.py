#!/usr/bin/env python3
"""Session 3: Ravenmoor — The Split"""
import requests, json, time, sys

BASE      = "http://localhost:3000/api"
DM_DEF_ID = "be6f7390-0953-4b8a-ba95-c1ff81694b3e"
CAMPAIGN  = "Ravenmoor"
SESSION   = "session3"

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
print("  RAVENMOOR — SESSION 3: THE SPLIT")
print("▓"*62)
print("\nThree threads. Everyone alone. Isolation penalties apply.\n")

# ── THREAD A: DORIAN + MINE RECORDS ──────────────────────────────
hr("THREAD A — Dorian and the Mine Records")
turn(
    "Dorian Ashgrove slips into the Aldwick Inn's back room while Marta is still asleep. "
    "Read rules/investigation.md — the mine records are here, stolen from the Manor. "
    "DC 13 Investigation to find the relevant entries (Investigation +5). Roll it. "
    "If found: the records describe what the miners broke through six months before the flood — "
    "a sealed chamber, pre-dating the mine by centuries. The foreman's notes say: "
    "'Sound from below the seal. Like breathing. Decided not to report upward.' "
    "Three weeks later, the flood. The flood that drowned forty-three men. "
    "Dorian is alone. It is still dark outside. He hears Marta moving upstairs. "
    "He has maybe two minutes. What does he copy down? What does he photograph in his mind? "
    "He is a disgraced investigator — this is the moment he stops being disgraced. "
    "Play the urgency. Apply isolation penalty per rules/fear_and_dread.md.",
    "turn01a_dorian_mine_records.md"
)

# ── THREAD B: VERA + LENA ─────────────────────────────────────────
hr("THREAD B — Vera and Lena Marsh")
turn(
    "Vera Nighthollow returns to Lena's cottage alone at first light. "
    "Read world/npcs.md for Lena — she doesn't know she's a ghost, or something near it. "
    "Vera doesn't approach it as a threat. She sits across from Lena and asks directly: "
    "'When did you last leave this house?' "
    "Lena thinks about it for a long time. 'Last Tuesday.' Then: 'What year is it?' "
    "Vera tells her. Lena's expression doesn't change but something behind her eyes does. "
    "She asks Vera what she is — meaning witch, not human. Vera tells her that too. "
    "Lena says: 'My father wants something. From the people who came. He said to say: "
    "the thing in the well is not from the mine. The mine broke through to it. "
    "It was already there. It has always been there.' "
    "Roll DC 14 Arcana for Vera (Arcana +5) to understand what kind of entity this describes. "
    "If she succeeds: this is older than the village. Older than the marsh. "
    "It doesn't want to harm anyone. It wants to be answered. It has been unanswered for centuries. "
    "Write Vera sitting with that information in the backwards-clock cottage.",
    "turn02b_vera_lena.md"
)

# ── THREAD C: ISOLDE + EMMETT + FATHER VANE ──────────────────────
hr("THREAD C — Isolde, Emmett, Father Vane")
turn(
    "Isolde leads Emmett to Father Vane's chapel before dawn. Read world/npcs.md for Father Vane — "
    "gaunt, trembling, hasn't slept since he performed the sealing rite. "
    "He takes one look at Emmett and knows immediately. 'You went into the well.' "
    "He doesn't ask how. He goes to the vestry and comes back with a cloth-wrapped bundle: "
    "old iron nails (the same kind Old Tomas carries) and a vial of what smells like seawater and ash. "
    "He says he can sever the connection — the same rite he used on the mine doors. "
    "But he tells them what it cost him: he doesn't dream anymore. He doesn't sleep. "
    "Something small was taken from him to seal the mine. "
    "For Emmett it would be the same. Ask Emmett if he consents. "
    "Roll DC 14 Religion for Isolde (Religion +4) to assist Father Vane correctly. "
    "Play the rite: iron nails in a circle, the ash-water, Vane's voice shaking but certain. "
    "Roll DC 15 Wisdom for the rite to hold (aided by Isolde's check — advantage if she succeeded). "
    "Whatever the outcome: describe what leaves Emmett, or what doesn't.",
    "turn03c_sealing_rite.md"
)

# ── CONVERGENCE ──────────────────────────────────────────────────
hr("CONVERGENCE — The Three Threads Reconvene")
turn(
    "The three threads converge back at the Aldwick Inn at midmorning. "
    "Read session3/turn01a_dorian_mine_records.md, turn02b_vera_lena.md, "
    "turn03c_sealing_rite.md if available, else recall what happened in this session. "
    "They each report what they found. Play this as information dropping in sequence: "
    "Dorian first: the mine broke through a sealed pre-existing chamber. Sound like breathing. Forty-three dead. "
    "Vera next: the entity is ancient, predates the village, has been unanswered for centuries. It just wants to be heard. "
    "Then Isolde and Emmett — whatever the sealing rite produced. "
    "The silence after all three have spoken. "
    "Then Dorian says the thing no one wants to say: "
    "'If it only wants to be answered — what happens if we answer it?' "
    "No one responds. Marta brings coffee without being asked. She heard. "
    "She sets the cups down and says: 'The last person who answered it was my cousin Casimir. "
    "He's been in the east tower for three weeks.' "
    "End the session on that sentence.",
    "turn04_convergence.md"
)

hr("SESSION 3 COMPLETE")
print("Files written to glorfindel-data/campaigns/Ravenmoor/session3/")
print("Index: glorfindel-data/campaigns/Ravenmoor/session3/TURNS.md")
print("\nState of play:")
print("  • The entity predates the village. It wants to be answered.")
print("  • Casimir Voss answered it. He's been in the east tower ever since.")
print("  • The mine records confirm a sealed pre-existing chamber.")
print("  • Emmett is either freed or still compromised.")
print("\nSession 4 hook: they have to go to the manor.")

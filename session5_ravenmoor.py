#!/usr/bin/env python3
"""Session 5: Ravenmoor — The Monster Unleashed"""
import requests, time, sys

BASE      = "http://localhost:3000/api"
DM_DEF_ID = "be6f7390-0953-4b8a-ba95-c1ff81694b3e"
CAMPAIGN  = "Ravenmoor"
SESSION   = "session5"

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
                print(prose[:1600])
                if len(prose) > 1600:
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
print("  RAVENMOOR — SESSION 5: THE MONSTER UNLEASHED")
print("▓"*62)
print()
print("Three days after Session 4.")
print("Casimir is at the manor. Marta's well is dry.")
print("The entity was never the threat.")
print()

# ── TURN 1: THE MORNING ──────────────────────────────────────────
hr("TURN 1 — Three Days Later: The Note and the Dry Well")
turn(
    "Read session4/turn06_choice.md — the party returned to the Aldwick Inn in silence. "
    "Three days have passed. Ravenmoor is quieter than before. Wrong kind of quiet. "
    "Two things happen simultaneously at dawn: "
    "FIRST: a sealed letter arrives from Casimir Voss at the manor — delivered by a stable boy "
    "who looks like he ran the whole way. The letter: 'I have been remembering things from the tower. "
    "Not all of them are mine. Come at first light. Do not bring Father Vane. — C.V.' "
    "SECOND: Old Tomas appears at the inn door before breakfast. He never comes to the inn. "
    "He stands in the doorway holding an empty bucket. He says: "
    "'Eastern well ran dry this morning. First time in living memory. '  "
    "He sets the bucket down. He says: 'The water went somewhere.' "
    "Read world/npcs.md for Old Tomas — gravedigger, iron nails, knows more than he says. "
    "He looks at the bucket for a long moment. He says: 'It went down.' "
    "Roll DC 13 Insight for Dorian (Investigation +5) to read what Tomas isn't saying. "
    "If he succeeds: Old Tomas has seen this before. Once. The week before the mine flood. "
    "Play the inn at dawn: the letter on the table, the bucket on the floor, Old Tomas in the doorway. "
    "The party understanding before anyone says it: they have to go to the mine.",
    "turn01_morning.md"
)

# ── TURN 2: FATHER VANE'S CONFESSION ────────────────────────────
hr("TURN 2 — What Father Vane Sealed")
turn(
    "Read session4/turn03_tower_climb.md and session3/turn03c_sealing_rite.md — "
    "Father Vane has performed two sealing rites: the mine doors, and Casimir. "
    "The party goes to the chapel before the mine. Father Vane is there. "
    "He is awake. He has been awake. "
    "When they tell him about the dry well, his face does something complicated. "
    "He sits down heavily in the front pew. He says: "
    "'I sealed the mine doors eleven years ago. After the flood. I thought I was sealing the entity — "
    "the thing that had been calling the miners down. I nailed the iron. I spoke the rite. "
    "The calling stopped. I thought that was the proof.' "
    "He stops. He looks at his hands — the ones that no longer dream. "
    "'But the entity in the well — the thing Vera said only wanted to be answered — "
    "if that was never the threat... then what was I sealing *in*?' "
    "Roll DC 15 Arcana for Vera (Arcana +5) to connect what she knows: "
    "the entity was pre-human, non-malevolent, question-shaped. It didn't flood the mine. "
    "If she succeeds: the entity wasn't down there. It was at the boundary. "
    "It was asking its question — 'what is the thing that existed before you knew you existed' — "
    "because the answer IS the thing in the mine. "
    "The entity was the warning. Father Vane sealed the warning out and the monster in. "
    "Apply dread. Play Father Vane understanding eleven years too late what he actually did.",
    "turn02_father_vane.md"
)

# ── TURN 3: THE MINE ─────────────────────────────────────────────
hr("TURN 3 — The Mine Entrance")
turn(
    "Read session3/turn01a_dorian_mine_records.md — the mine records Dorian found: "
    "a sealed pre-existing chamber, sound like breathing, foreman didn't report it. "
    "Three weeks later the flood. Forty-three dead. "
    "The mine entrance is a thirty-minute walk northeast of Ravenmoor. "
    "Read world/setting.md for the landscape. "
    "Father Vane insists on coming despite being told not to. Casimir's letter said don't bring him. "
    "The mine entrance: a stone arch, the shaft going down at forty-five degrees, "
    "the doors sealed with iron nails — the same nails Old Tomas carries, the same Father Vane used on Casimir. "
    "The nails are rusted through. Not age — something ate through them from the inside. "
    "The doors are ajar. Two inches. Just enough that something cold comes out. "
    "Roll DC 15 Perception for the party (best: Vera, Perception +3). "
    "If they succeed: the smell from inside is not mine-smell — damp rock and coal dust. "
    "It is the smell of the eastern well. The same water. The same depth. Connected. "
    "Father Vane looks at the rusted nails on the ground and says nothing. "
    "He picks one up. He holds it. It crumbles in his hand. "
    "Play the moment before they go in: the open doors, the rusted iron, "
    "the cold air that smells like the well that ran dry this morning.",
    "turn03_mine_entrance.md"
)

# ── TURN 4: THE DESCENT ──────────────────────────────────────────
hr("TURN 4 — Underground")
turn(
    "Read rules/fear_and_dread.md — underground isolation, no natural light, enclosed spaces. "
    "Apply dread rules for the descent. All isolation penalties double underground. "
    "The mine shaft: forty-three years of abandonment. Timber supports holding. Barely. "
    "Evidence as they descend: "
    "— At thirty feet: a miner's boot. Just one. No sign of the foot that was in it. "
    "— At sixty feet: the walls are wet. The water table has risen. "
    "Same seep-wet from the manor east tower foundation in session4/turn01_approach.md. "
    "— At ninety feet: they find the foreman's work station. "
    "Read session3/turn01a_dorian_mine_records.md — Dorian photographed the records. "
    "The foreman's desk is still here. His coffee cup. His logbook open to the last entry: "
    "'Day 847. Sound from below the new shaft. Not machinery. Not settling. Like something "
    "large adjusting its position. Decided not to report upward. Will check tomorrow.' "
    "Tomorrow never came. "
    "Roll DC 14 Fear Save for everyone (Wisdom) at ninety feet. "
    "Emmett with advantage if the sealing rite held; Father Vane automatically fails — "
    "he's been here before. He sealed these doors. He thought it was over. "
    "The shaft continues down. The pre-existing chamber is forty feet deeper. "
    "They can hear it from here. Not breathing. Not movement. "
    "The sound of something that has been waiting.",
    "turn04_descent.md"
)

# ── TURN 5: THE CHAMBER ──────────────────────────────────────────
hr("TURN 5 — The Sealed Chamber")
turn(
    "The pre-existing chamber. Read session3/turn01a_dorian_mine_records.md — "
    "'a sealed chamber, pre-dating the mine by centuries.' "
    "What the party finds: not a natural cave. It was built. "
    "Stone walls, dressed and fitted. Pre-Roman. Pre-anything they can name. "
    "The chamber is circular. In the center: a shallow depression, "
    "thirty feet across, filled with water. "
    "The water from the eastern well. All of it. It came here. "
    "And in the water: something large and dark and still. "
    "Roll DC 18 Arcana for Vera (+5) to understand what she is looking at. "
    "Roll DC 16 Religion for Isolde (+4) to recognise the chamber's purpose. "
    "If Vera succeeds: this is not the entity from the well. "
    "The entity is warm. Question-shaped. Ancient but not hostile. "
    "This is something else. Something that was here before the entity. "
    "Before the village. Before the marsh. Before the language you'd use to name it. "
    "The entity spent centuries asking its question at the boundary because this is what "
    "'the thing that existed before you knew you existed' actually IS. "
    "If Isolde succeeds: the chamber is a seal, not a prison. There's a difference. "
    "A prison keeps something in. A seal keeps something contained — as long as the seal is maintained. "
    "Father Vane's iron nails were the maintenance. And they have rotted through. "
    "The thing in the water is already awake. It has been awake since the nails rusted. "
    "It is looking at them. It has been looking since they entered the mine. "
    "Play the chamber: the dressed stone, the water, the thing in it, Father Vane on his knees.",
    "turn05_chamber.md"
)

# ── TURN 6: THE MONSTER UNLEASHED ───────────────────────────────
hr("TURN 6 — The Monster Unleashed")
turn(
    "Read session4/turn05_entity_speaks.md — the entity asked: "
    "'What is the name of the thing that existed before you knew you existed?' "
    "They are standing in the answer. "
    "The thing in the water moves. "
    "Not violent — purposeful. The way a door opens. The way a held breath releases. "
    "It rises. It is not monstrous in shape — it is monstrous in the way a drought is monstrous, "
    "or a flood. It has no malice. It has no mercy. It simply IS, and it has been contained, "
    "and now it is not. "
    "Roll Initiative for the party. This is not a combat — it cannot be fought. "
    "Roll DC 17 Wisdom Save for everyone in the chamber. "
    "Anyone who fails: they understand completely, for one terrible moment, what this thing is. "
    "They lose 1d6 Sanity points (treat as Dread per rules/fear_and_dread.md). "
    "The only way out is up. Forty feet of shaft, ninety feet of mine, Father Vane who cannot run. "
    "As they move: the water in the chamber rises. The thing doesn't chase — it expands. "
    "The way water finds its level. It is going to fill the mine, and then the well, "
    "and then the water table of Ravenmoor itself. "
    "Roll Athletics DC 14 for everyone to get Father Vane up the shaft (he is old, he is broken). "
    "They make it out — or most of them do, on the dice. "
    "They stand at the mine entrance in grey daylight. Behind them: the sound of water. Rising. "
    "In the direction of Ravenmoor, a mile away: the eastern well begins to overflow. "
    "It runs uphill. Water doesn't run uphill. "
    "End the session here. They are standing outside the mine in the daylight "
    "and Ravenmoor is a mile away and the water is moving toward it "
    "and they have maybe two hours. "
    "Dorian says: 'What do we do.' "
    "It is not a question. "
    "Vera says: 'We answer it.'",
    "turn06_unleashed.md"
)

hr("SESSION 5 COMPLETE")
print("Files written to glorfindel-data/campaigns/Ravenmoor/session5/")
print("Index: glorfindel-data/campaigns/Ravenmoor/session5/TURNS.md")
print()
print("THE REVEAL:")
print("  The entity was the warning. It spent centuries asking one question.")
print("  'What is the name of the thing that existed before you knew you existed?'")
print("  The answer is in the mine. The party just let it out.")
print()
print("State of Ravenmoor:")
print("  • The monster is loose. It moves like water. It cannot be fought.")
print("  • The eastern well is overflowing uphill. Two hours.")
print("  • Father Vane is alive. Barely. He understands what he sealed eleven years ago.")
print("  • The entity — the question-spirit — is still there. Still at the boundary.")
print("  • Vera said: 'We answer it.' She has a plan.")
print()
print("Session 6: THE ANSWER")
print("  One final session. They have to give the monster a name.")
print("  The entity has been asking for centuries because naming it is the seal.")
print("  The iron nails were a temporary measure. The real seal is a true name.")
print("  No one has ever known the name. Until now.")
print("  Casimir's letter. The things he remembered in the tower that weren't his.")
print("  He heard it. In three weeks of answering questions, the entity told him.")
print("  Casimir Voss knows the name of the thing in the mine.")

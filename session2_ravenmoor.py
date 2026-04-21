#!/usr/bin/env python3
"""Session 2: Ravenmoor — What Lives in the Well"""
import requests, json, time, sys

BASE      = "http://localhost:3000/api"
DM_DEF_ID = "be6f7390-0953-4b8a-ba95-c1ff81694b3e"
CAMPAIGN  = "Ravenmoor"
SESSION   = "session2"

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
            # Print the prose
            prose_path = (
                f"glorfindel-data/campaigns/Ravenmoor/{SESSION}/"
                + (result.get("output_file","").split("/")[-1] if isinstance(result, dict) else out_file)
            )
            try:
                with open(prose_path) as f:
                    prose = f.read()
                print(prose[:1200])
                if len(prose) > 1200:
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

# ──────────────────────────────────────────────────────────────
print("\n" + "▓"*62)
print("  RAVENMOOR — SESSION 2: WHAT LIVES IN THE WELL")
print("▓"*62)
print("\nPicking up at dawn. The party stands at the eastern well.")
print("The rain has stopped. The mill wheel is turning.\n")

# ── TURN 1 ────────────────────────────────────────────────────
hr("TURN 1 — The Well, Up Close")
turn(
    "Dawn. The party returns to the eastern well. Read session1/beat09_eastern_well.md if it exists, "
    "else read session1/ files to recall what happened. Also read rules/investigation.md. "
    "Dorian crouches at the stone lip and examines the scratch marks more carefully — "
    "fingernails, going down, not up. At least four different people. "
    "The water below is dark and perfectly still despite a light wind. "
    "Vera kneels beside him and lets her Detect Magic run. Roll DC 17 Arcana for Vera (Arcana +5). "
    "If she succeeds: the magic below isn't undead animation — it's compulsion. "
    "Something down there is *calling*. "
    "Isolde recognises the sensation from one of the old rites Father Vane spoke of last night. "
    "She goes pale. Describe the well, the marks, and the dawning horror of what they mean.",
    "turn01_well_close.md"
)

# ── TURN 2 ────────────────────────────────────────────────────
hr("TURN 2 — Emmett Goes Down")
turn(
    "Emmett volunteers to be lowered into the well on the rope. No one is surprised. "
    "Read rules/fear_and_dread.md — Emmett is immune to Echo fear saves but not compulsion. "
    "Dorian and Isolde hold the rope. Vera watches with Detect Magic active. "
    "Roll Athletics DC 12 for Emmett to descend safely (Athletics +4). "
    "As he goes down: the air gets colder. The walls are scratched all the way down — "
    "desperate, deep, going deeper the further he descends. "
    "At about twenty feet, his lantern illuminates something in the water: "
    "a shape, roughly human, perfectly still, staring up at him from beneath the surface. "
    "It doesn't move. It doesn't breathe. It doesn't blink. "
    "Roll DC 16 Fear Save for Emmett (Wisdom +1). Even he might flinch at this. "
    "Whatever it is, it mouths a single word at him. He can't hear it — but he can read it: "
    "'Come.' Pull him up immediately.",
    "turn02_emmett_descends.md"
)

# ── TURN 3 ────────────────────────────────────────────────────
hr("TURN 3 — The Mill House and Lena Marsh")
turn(
    "Shaken, the party retreats from the well. As they pass the Millhouse, the wheel slows. "
    "Read world/npcs.md for Lena Marsh. "
    "Lena is standing in her doorway. She wasn't there a moment ago. "
    "She looks exactly nineteen. She's been standing in the rain and she isn't wet. "
    "She speaks first: 'You went to the well.' Not a question. "
    "Vera rolls Insight DC 13 (Insight +4) — what does she sense about Lena? "
    "If Vera succeeds: Lena is not lying but she is not entirely *present*. "
    "Part of her is somewhere else. Part of her has been somewhere else for a very long time. "
    "Lena tells them her father leaves food sometimes — or something does. "
    "She invites them inside for tea. Old Tomas specifically warned against this. "
    "Play this scene: who goes in, who hesitates, what Dorian sees in his investigator's eye "
    "when he looks at the cottage door. Apply dread rules if appropriate.",
    "turn03_lena_marsh.md"
)

# ── TURN 4 ────────────────────────────────────────────────────
hr("TURN 4 — Inside the Marsh Cottage")
turn(
    "The party steps inside Lena's cottage (at least some of them — play the hesitation). "
    "Read rules/fear_and_dread.md and world/npcs.md. "
    "The cottage is wrong in small ways: the clock on the mantel runs backwards. "
    "A place at the table is set but the food on it is from last week and untouched but not rotted. "
    "A coat on the hook is a miner's coat — the same type as the Echo Emmett saw. "
    "Lena makes tea without appearing to move her hands. "
    "She tells them about her father: 'He went down with the others. But he still comes home. "
    "He's just quieter now.' She says this with complete sincerity. "
    "Roll DC 13 Fear Save for anyone inside (Wisdom). "
    "Then she says something that stops them cold: "
    "'He told me there's a new one now. Just started listening. He said to tell whoever came — "
    "don't answer it. Once you answer, it knows your voice.' "
    "She looks at Emmett. 'You already went down, didn't you.' "
    "Play Emmett's reaction. Play the party's reaction. This is bad.",
    "turn04_cottage_interior.md"
)

# ── TURN 5 ────────────────────────────────────────────────────
hr("TURN 5 — Emmett Hears Something")
turn(
    "That evening back at the inn. Read rules/fear_and_dread.md — Emmett has been quiet since the well. "
    "Dorian is writing in his notebook: the word the shape mouthed was 'Come' — "
    "the same word as the letter that brought them all here. He stares at this for a long time. "
    "At midnight, Emmett wakes up. He didn't mean to. "
    "He's sitting upright in bed and he's already dressed and he doesn't remember doing it. "
    "His boots are pointing toward the door. Toward the east. "
    "Roll DC 14 Wisdom Save for Emmett (Wisdom +1) to resist the compulsion pull. "
    "If he fails: he's standing outside before he realises it, three steps toward the well, "
    "and it takes Vera grabbing his arm to stop him — she was watching from the window. "
    "If he succeeds: he wakes fully, sweating, boots facing east, and knows exactly what almost happened. "
    "Either way: apply 1 Dread Point to Emmett. "
    "The thing in the well knows his voice now. "
    "Write this as quiet, creeping horror — no monsters, just the wrongness of a man "
    "walking toward his own drowning in his sleep.",
    "turn05_emmett_midnight.md"
)

# ── TURN 6 ────────────────────────────────────────────────────
hr("TURN 6 — Morning Council: What Do We Do About Emmett")
turn(
    "Morning. The party gathers before dawn in the common room — Marta isn't awake yet. "
    "They lay out what they know. Read session1/ files for context on the hooks. "
    "They have a problem: Emmett is compromised. The thing in the well has his voice. "
    "They need to either cut the connection or go back down with a plan. "
    "Read world/npcs.md — Isolde thinks Father Vane might know a sealing rite. "
    "Dorian wants to find the mine records (mentioned in rules/investigation.md as stolen from the Manor, "
    "now in the inn's back room). Vera thinks Lena Marsh knows more than she said. "
    "Play the council: three directions, one compromised party member, dawn coming fast. "
    "They decide to split: Dorian goes for the mine records in the back room while Marta is asleep, "
    "Vera goes back to Lena, Isolde takes Emmett to Father Vane. "
    "Each split increases isolation penalty per rules. "
    "End on them separating in the grey pre-dawn light, each heading a different direction. "
    "Write this as the moment the campaign shifts from investigation to urgency.",
    "turn06_morning_council.md"
)

hr("SESSION 2 COMPLETE")
print("Files written to glorfindel-data/campaigns/Ravenmoor/session2/")
print("Reasoning traces in session2/.meta/")
print("\nThe situation:")
print("  • Emmett is compromised — the thing in the well knows his voice")
print("  • Lena Marsh is a ghost who doesn't know she's a ghost")
print("  • Mine records are in the inn's back room")
print("  • The party has split — isolation penalties apply to everyone")
print("\nSession 3 hooks:")
print("  1. Dorian and the mine records — what did they break through?")
print("  2. Vera and Lena — what does her father actually want?")
print("  3. Isolde, Emmett, Father Vane — can the sealing rite be reversed?")

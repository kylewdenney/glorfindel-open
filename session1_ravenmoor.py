#!/usr/bin/env python3
"""Session 1: Ravenmoor — The Letter (uses /session API)"""
import requests
import json
import time
import sys

BASE       = "http://localhost:3000/api"
DM_DEF_ID  = "be6f7390-0953-4b8a-ba95-c1ff81694b3e"
CAMPAIGN   = "Ravenmoor"
SESSION    = "session1"

def turn(intent, output_file=None, beat_label=""):
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
    print(f"  → {out_file}  (task {task_id})")

    for _ in range(120):
        time.sleep(5)
        s = requests.get(f"{BASE}/tasks/{task_id}").json()
        status = s.get("status", "")
        if status in ("complete", "completed", "failed", "error"):
            resp = s.get("response") or {}
            result = resp.get("result") or s.get("result")
            if isinstance(result, dict):
                print(f"  ✓ {result.get('output_file','?')}  |  {result.get('action_summary','')[:120]}")
            return result
        sys.stdout.write(".")
        sys.stdout.flush()
    print(" [timeout]")
    return None

def hr(title):
    print(f"\n{'='*60}\n  {title}\n{'='*60}")

# ── SESSION 1 ─────────────────────────────────────────────────────
print("\n" + "█"*60)
print("  RAVENMOOR — SESSION 1: THE LETTER  (Session API)")
print("█"*60)

hr("BEAT 1: Arrival at the Crossed Road")
turn(
    "Session 1 begins. Read session1/session1_hook.md and world/setting.md. "
    "The party — Sister Isolde Carrow, Dorian Ashgrove, Emmett Grave, Vera Nighthollow — "
    "have just been dropped by the coachman at the Crossed Road junction outside Ravenmoor. "
    "Dusk, early November, rain for 11 days. Describe the arrival: the coachman's warning, "
    "the sign scratched with '11 DAYS', the village dim in the distance. Set the gothic horror tone.",
    "beat01_arrival.md"
)

hr("BEAT 2: Walking Into Ravenmoor")
turn(
    "The party walks toward Ravenmoor. The flooded road, the Ashfen Marshes smell, "
    "the mill wheel turning with no wind, smoke from only a few chimneys. "
    "Locals peer from windows and shut curtains. A dog barks once and goes silent. "
    "They reach the Aldwick Inn — warm light inside, sign creaking, voices going quiet as the door opens.",
    "beat02_approach.md"
)

hr("BEAT 3: Meeting Marta Voss")
turn(
    "The party enters the Aldwick Inn and asks for Marta. Read world/npcs.md for her character. "
    "Marta Voss — iron-grey, stout, suspicious — studies each of them before speaking. "
    "She confirms she was expecting them, settles them at a corner table, brings food unbidden. "
    "First hint: 'Three gone in a fortnight. Casimir hasn't come down from the manor since his father "
    "went in the ground.' She won't say more yet. Write her dialogue with edge and wariness.",
    "beat03_marta_voss.md"
)

hr("BEAT 4: Old Tomas in the Common Room")
turn(
    "The party tries to gather information. Four other patrons including Old Tomas — the gravedigger, "
    "iron nails clinking in his coat. Read world/npcs.md. Dorian Ashgrove (Persuasion +4) approaches; "
    "roll DC 12 Persuasion with -2 outsider penalty. Tomas speaks in fragments: "
    "'Don't go near the east well. Don't go to the mill after dark. Don't let the girl "
    "in the marsh house invite you inside.' He won't explain. Apply fear rules if appropriate.",
    "beat04_old_tomas.md"
)

hr("BEAT 5: Father Vane Confesses")
turn(
    "Father Dorian Vane arrives at the inn, agitated. Read world/npcs.md — gaunt, trembling hands, "
    "smells of incense and something underneath. He sent the letters. He confesses it. "
    "Tells them about Casimir: lights in the east tower, strange sounds, no communication since the funeral. "
    "Names the three missing: Bren the tanner, old Mirra, young Pell — all drew from the eastern well. "
    "He starts to mention the mine, then stops himself. Hands shaking badly.",
    "beat05_father_vane.md"
)

hr("BEAT 6: The Echo — Emmett Sees the Miner")
turn(
    "That night, Emmett Grave goes outside alone — Isolation Penalty applies per rules/fear_and_dread.md. "
    "He sees an Echo: a miner in work clothes, soaked, standing at the property edge staring east. "
    "It doesn't move or speak. Water drips from it but leaves no puddle. Then it's gone. "
    "Emmett is immune to Echo Fear Saves per the rules. Describe the vision viscerally. "
    "He returns and tells the party. Vera uses Detect Magic (at will) — DC 14 Arcana to interpret the aura. "
    "Roll for Vera (Arcana +5).",
    "beat06_echo_miner.md"
)

hr("BEAT 7: The Party Deliberates")
turn(
    "Read session1/session1_hook.md for session goals. The party debates: "
    "1. Eastern well (missing villagers drew from it), "
    "2. Casimir Voss manor (lights in tower), "
    "3. Lena Marsh's house by the Millhouse. "
    "Isolde wants the funeral and Casimir; Dorian wants the well — physical evidence; "
    "Emmett is drawn to the manor's buried history; Vera wants Lena Marsh — the echo pointed east. "
    "They settle on the eastern well at dawn. Write the debate with each character's distinct voice.",
    "beat07_deliberation.md"
)

hr("BEAT 8: Night at the Inn")
turn(
    "The party sleeps at the inn. Per rules/fear_and_dread.md a full rest in safe lit place removes 1 Dread. "
    "Each character's night: Isolde hears slow deliberate footsteps on the roof. "
    "Dorian writes notes, at 3am his candle goes out on its own. "
    "Emmett sleeps but wakes with his boots facing the wrong direction. "
    "Vera sits up all night — hears the mill wheel turning in the distance. "
    "Morning: grey, cold. The rain has stopped — for now.",
    "beat08_night_inn.md"
)

hr("BEAT 9: The Eastern Well")
turn(
    "Dawn. The party goes to the eastern well. Read rules/investigation.md for clue tiers. "
    "Old stone, iron ring, rope gone greenish. Water smells wrong. "
    "Surface clue (automatic): rope has fresh abrasions — recently used. "
    "Hidden clue DC 13: Dorian (Investigation +5) searches carefully — roll it. "
    "If found: scratch marks inside the stone lip, like fingernails going down, several sets. "
    "Deep clue DC 17: Vera (Arcana +5) with Detect Magic — roll it. "
    "If found: something at the bottom that is not water. It is aware of them. "
    "The water surface ripples once, against the wind. End before they can process it.",
    "beat09_eastern_well.md"
)

hr("BEAT 10: Session Close")
turn(
    "End of Session 1. The party walks back toward the inn aware that something in the well was watching. "
    "As they pass the Millhouse, the wheel stops — just for a moment — then starts again. "
    "Lena Marsh stands at her window watching them. She raises one hand — not a wave. "
    "Close on dread and anticipation in 2 short atmospheric paragraphs. "
    "Then write a summary of the session: NPCs met, clues found, dread points, "
    "and the three open hooks for Session 2: the well, Casimir, Lena Marsh.",
    "beat10_session_close.md"
)

hr("SESSION 1 COMPLETE")
print("Files written to glorfindel-data/campaigns/Ravenmoor/session1/")
print("Reasoning traces in session1/.meta/")
print("\nOpen hooks for Session 2:")
print("  1. The eastern well — something aware at the bottom")
print("  2. Casimir Voss — east tower light, changed since the funeral")
print("  3. Lena Marsh — watching, her 'father' drowned a century ago")

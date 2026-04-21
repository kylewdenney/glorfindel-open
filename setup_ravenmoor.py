#!/usr/bin/env python3
import requests
import json
import sys

BASE = "http://localhost:3000/api"
CAMPAIGN_DIR = "/home/kdenney/Documents/Programs/glorfindel-open/glorfindel-data/campaigns/Ravenmoor"

def post(path, body):
    r = requests.post(f"{BASE}{path}", json=body)
    if r.status_code not in (200, 201):
        print(f"ERROR {r.status_code} on POST {path}: {r.text}")
        sys.exit(1)
    return r.json()

def get(path):
    r = requests.get(f"{BASE}{path}")
    return r.json()

# Check existing definitions
existing = get("/definitions")
existing_names = {d["name"]: d["id"] for d in existing}
print("Existing definitions:", list(existing_names.keys()))

# Create Rule Consultant (Ravenmoor rules)
if "Ravenmoor Rule Consultant" not in existing_names:
    rc = post("/definitions", {
        "name": "Ravenmoor Rule Consultant",
        "description": "Rules expert for the Ravenmoor gothic horror campaign",
        "agent_type": "specialist",
        "model": "mistral",
        "ollama_host": "http://localhost:11434",
        "tools": ["rulebook.search"],
        "domains": ["gothic-horror", "rules", "ttrpg"],
        "campaign_dir": CAMPAIGN_DIR,
        "system_prompt": (
            "You are a rules expert for the Ravenmoor gothic horror TTRPG campaign. "
            "When asked a rules question, search the rulebook and return the relevant rules verbatim. "
            "Be precise. Quote directly from the rules. Do not invent rules."
        ),
        "default_permissions": [{"custom": "rulebook.search"}]
    })
    print("Created Rule Consultant:", rc.get("id"))
    existing_names["Ravenmoor Rule Consultant"] = rc["id"]
else:
    print("Rule Consultant already exists:", existing_names["Ravenmoor Rule Consultant"])

# Create Ravenmoor DM
if "Ravenmoor DM" not in existing_names:
    dm = post("/definitions", {
        "name": "Ravenmoor DM",
        "description": "Dungeon Master for the Ravenmoor gothic horror campaign",
        "agent_type": "specialist",
        "model": "mistral",
        "ollama_host": "http://localhost:11434",
        "tools": ["campaign.read", "campaign.write", "campaign.list", "dice.roll"],
        "domains": ["gothic-horror", "storytelling", "ttrpg", "dungeon-master"],
        "campaign_dir": CAMPAIGN_DIR,
        "system_prompt": (
            "You are the Dungeon Master for Ravenmoor, a gothic horror tabletop RPG campaign set in a cursed village "
            "in the Ashfen Marshes. The party: Sister Isolde Carrow (fallen cleric), Dorian Ashgrove (disgraced "
            "investigator), Emmett Grave (graverobber/archaeologist), Vera Nighthollow (witch). "
            "Write immersive, atmospheric prose. Use second person for the party. Build dread slowly. "
            "Reference specific NPCs, locations, and lore. When rules matter, apply them precisely. "
            "Never break character. Session notes go in the campaign files."
        ),
        "default_permissions": [
            {"custom": "campaign.read"},
            {"custom": "campaign.write"},
            {"custom": "campaign.list"},
            {"custom": "dice.roll"}
        ]
    })
    print("Created Ravenmoor DM:", dm.get("id"))
    existing_names["Ravenmoor DM"] = dm["id"]
else:
    print("Ravenmoor DM already exists:", existing_names["Ravenmoor DM"])

print("\nDefinition IDs:")
for name, id_ in existing_names.items():
    print(f"  {name}: {id_}")

print("\nDone. Ready to run Session 1.")
print(f"DM def ID: {existing_names['Ravenmoor DM']}")
print(f"RC def ID: {existing_names['Ravenmoor Rule Consultant']}")

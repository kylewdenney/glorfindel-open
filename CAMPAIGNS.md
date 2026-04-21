# Playing a Campaign

Glorfindel runs TTRPG campaigns entirely on your machine using local LLMs. No cloud, no subscription, no one watching.

## Quick Start

```bash
git clone https://github.com/kylewdenney/glorfindel-open.git
cd glorfindel-open

# Pull a model (mistral is the default, ~4GB)
ollama pull mistral

cargo build --bin glorfindel-admin
GLORFINDEL_DATA_DIR=./glorfindel-data ./target/debug/glorfindel-admin
```

Open `http://localhost:3000/campaign`.

---

## The Campaigns

### Ravenmoor — Gothic Horror

Something is wrong in the village of Ravenmoor. The eastern well ran dry three days ago. Nobody will talk about the mine.

- **Tone:** slow dread, folk horror, secrets
- **Mode:** DM mode — the system runs sessions autonomously
- **Sessions 1–7 complete** — load any session and run forward, or start fresh from session 1

### Avalon — Gaelic Fae Arthurian

Your ancestors served the Round Table. You've just found out that was real — the Fae contracts, the living myths, all of it. Morgan le Fay is watching. Avalon is bleeding through.

- **Tone:** mythic, eerie, high stakes
- **Mode:** Play Mode — you are the characters
- **Session 1 complete, Session 2 opening written**
- **Four pre-generated characters:**

| Character | Lineage | Devotion | Best At |
|-----------|---------|----------|---------|
| Caius ap Llywarch | Lancelot | THE SWORD | BLADE +4 |
| Elen ferch Maelog | Morgana | THE VEIL | VEIL +4 |
| Peredur map Gwrtheyrn | Percival | THE GRAIL | LORE +3 |
| Sioned | Unknown | THE HUNT | OFFERING +3 |

---

## How to Play (Play Mode)

1. Select **Avalon** in the dashboard
2. Click **Play** in the top-right to switch to Play Mode
3. Pick your character from the party cards
4. Type what your character does in the action box
5. Hit **Take Action** — the pipeline rolls dice, applies your Devotion bonus, and writes the scene

The six checks are: **BLADE** (combat), **PRESENCE** (persuasion/intimidation), **LORE** (knowledge/history), **RITUAL** (Fae magic), **OFFERING** (bargaining with the Fae), **VEIL** (perception of the hidden world).

Your **Devotion** gives you a bonus to one check. It also has a demand — the Fae remember their contracts.

---

## How to Run a DM Session (Ravenmoor)

1. Select **Ravenmoor** in the dashboard
2. Pick a session
3. Hit **Run Session Turn** — the pipeline handles everything
4. Read the output in the turn file viewer
5. Hit **Summarize** when the session feels complete
6. Use **Grand Opener** to write the opening of the next session

---

## The Logs

Every turn writes two files:

- `session1/turn_003.md` — the story output, plain prose
- `session1/.meta/turn_003.log` — the full pipeline trace: what each agent saw, what it decided, what it rolled

Click any `.meta` log in the dashboard to read it as a conversation. You can see exactly why the DM made every call.

---

## Bring Your Own Campaign

Drop a folder under `glorfindel-data/campaigns/your-campaign-name/` with:

```
world/
  setting.md    # world context fed to every pipeline stage
  party.md      # player characters with stats (for Play Mode)
  npcs.md       # recurring NPCs
rules/
  dice_and_abilities.md   # your check system
campaign/
  session1.md   # session hooks
session1/       # turns land here automatically
```

Select it in the dashboard. The pipeline reads your files. Start playing.

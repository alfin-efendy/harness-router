# Third-party pet assets

The pet spritesheets bundled under `apps/cockpit/public/pets/<slug>/sprite.webp`
were sourced from the [petdex](https://petdex.dev) community pet gallery
(manifest: `https://petdex.dev/api/manifest`, which redirects to
`https://assets.petdex.dev/manifests/petdex-v1.json`).

**No explicit license is published by petdex** for any submission — the manifest
and site carry no license metadata. Each sheet below was selected specifically
because it depicts a clearly generic, non-franchise design (a plant, a piece of
sports equipment, an office object, a generic animal) — never a franchise
character, celebrity likeness, or anything else trademark-adjacent. Inclusion
is per project-owner decision 2026-07-26, **with the following mitigation**:
these assets will be removed promptly on any submitter objection.

All six sheets were fetched 2026-07-26 and verified to be 1536x1872px WebP
images arranged as an 8x9 grid of 192x208px frames (see
`apps/cockpit/src/lib/pet-sprite.ts` and `.superpowers/sdd/task-3-report.md`
for the measurement).

| Slug | Display name | Submitted by | Source |
| --- | --- | --- | --- |
| `sprout` | Sprout | Chen W. | https://petdex.dev — https://assets.petdex.dev/pets/sprout-b2c16d8cc506/sprite.webp |
| `tennis-ball` | Tennis Ball | Haoran-Jie | https://petdex.dev — https://assets.petdex.dev/pets/tennis-ball-2f577e0b-ad5/sprite.webp |
| `crystal` | Crystal | Siugurd | https://petdex.dev — https://assets.petdex.dev/pets/crystal-566596b4f094/sprite.webp |
| `cloudlet` | Cloudlet | Wojciech W. | https://petdex.dev — https://assets.petdex.dev/pets/cloudlet-c1b1231a819b/sprite.webp |
| `boxcat` | Boxcat | railly | https://petdex.dev — https://assets.petdex.dev/curated/boxcat/spritesheet.webp |
| `docket-pin` | Docket Pin | codexfede | https://petdex.dev — https://assets.petdex.dev/pets/docket-pin-af6226fe080c/sprite.webp |

Fetch date for all entries above: **2026-07-26**.

**Removal note:** no explicit license published by the source; included per
project-owner decision 2026-07-26; will be removed promptly on any submitter
objection.

## Why these six

Petdex's manifest lists 4145 submissions across `character`, `creature`, and
`object` kinds; the vast majority of `character` entries (and a meaningful
slice of `creature`/`object` entries) are recognizable franchise or pop-culture
references. Only `object`/`creature`-kind entries with clearly generic,
original-looking designs were considered, and two additional candidates that
initially looked promising were rejected after visual inspection:

- `paperclip` (object, "Paperclip") — rejected: the sprite depicts an
  anthropomorphic paperclip with googly eyes, eyebrows, and gesturing arms —
  visually a direct pastiche of Microsoft's "Clippy" Office Assistant
  character, despite the generic slug/name.
- `cactus` (creature, "Cactus") — rejected: the sprite is a direct visual
  match for the "Cactus" plant character from the *Plants vs. Zombies* game
  franchise (cannon-mouth, red flower crown, matching art style/palette).

Several other slug matches for `clippy*` were skipped outright for the same
reason without downloading (obvious Clippy homages by name).

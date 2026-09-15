# Denpa Jack

[English](README.md) | [中文](README.zh.md)

Denpa Jack is Future Gadget No. 14 of the Future Gadget Laboratory (Mirai Gadget
Kenkyujo) — *Mirai Gajetto Jūyon-gōki: Denpa Jakku* (未来ガジェット14号機：電波ジャック).
In the story it forces recorded sound onto every screen in Akihabara; this project does
the useful half of that: **hold a hotkey, speak, and the words are forced into whatever
text field you are in** — transcribed, dictionary-corrected, optionally polished by an
LLM, then written through the accessibility API or a synthetic `Cmd+V`.

macOS menu-bar app (no Dock icon). While recording, an overlay shows live partial text
and a level meter.

## Features

- **Global hotkey** — `Fn`, modifier-only combos (e.g. `ctrl+option`), letters/digits/F1–F12;
  two activation modes (hold-to-talk / press-to-start, press-to-stop); record it in the UI
- **ASR engines** — Doubao streaming 2.0 (binary WebSocket, hotwords passed upstream, live
  partials) / Zhipu GLM-ASR (file-based, fallback) / OpenAI Realtime (module present)
- **Dictionary correction without an LLM** — variant replacement + regex normalisation +
  **pinyin fuzzy matching** (near-sound syllable ≈ 0.5 cost, only for ≥3-character terms),
  guarded so it cannot rewrite something already correct
- **LLM polish** — multiple profiles, **forced tool call for structured output** (never raw
  text), thinking switch and effort level, editable prompt with Markdown preview
- **Delivery** — accessibility write into the focused app first, `Cmd+V` as fallback;
  clipboard can either keep the transcript or restore what was there before
- **History** — JSONL records plus retained audio (play / re-transcribe / copy / reveal),
  trimmed to a configurable limit
- **Microphone** — device priority list, **event-driven** hot-plug refresh (no polling),
  live display of the device actually in use

## How it works

```
hotkey ──► cpal capture (native format → mono → 16 kHz, 200 ms chunks)
        ──► ASR engine (partials stream to the overlay)
        ──► release: finish + server-side finalisation
        ──► dictionary (normalise → variants → pinyin fuzzy)
        ──► optional LLM polish (tool call, failure degrades to dictionary text)
        ──► delivery: AX write → Cmd+V fallback (focus locked at press time)
        ──► clipboard + history (+ audio file, trimmed by limit)
```

## Build

```bash
npm install
npm run tauri dev          # development
```

Package for macOS:

```bash
npm run tauri build
# sign directly with the certificate file (no keychain, no trust needed)
~/.local/bin/rcodesign sign \
  --p12-file "certs/denpa-jack-dev-10y.p12" \
  --p12-password-file "certs/denpa-jack-dev-10y.pw" \
  "src-tauri/target/release/bundle/macos/Denpa Jack.app"
```

The first build needs network access: the keyboard layer uses a **patched `handy-keys`**
pinned by git revision — see [docs/DEPENDENCIES.md](docs/DEPENDENCIES.md).

## Code signing & permissions

Signing is done by [rcodesign](https://github.com/indygreg/apple-platform-rs) straight from a
`.p12` file — the certificate never has to be imported into a keychain and the system never
has to trust it. `codesign` cannot do that: with an untrusted self-signed certificate it
refuses to sign (`CSSMERR_TP_NOT_TRUSTED` / `no identity found`), while rcodesign produces a
standard signature that passes `codesign --verify` — and TCC permissions work with it
(verified end-to-end on this machine).

- Bundle id is fixed at `io.github.yangyaofei.denpajack`; the certificate is a 10-year
  self-signed `Denpa Jack Dev`, kept in the repo's `certs/` directory (gitignored, never committed;
  in CI it is injected from repository secrets — see [docs/CI-CD.md](docs/CI-CD.md)). Keep both
  unchanged — macOS TCC records permissions against a *code requirement* (`identifier` + certificate
  leaf), so a new bundle id or a new certificate means granting microphone / accessibility again.
- On **another Mac**: a copy made locally (USB, `scp`) runs directly but permissions must be
  granted there; a quarantined copy (AirDrop, browser, mail) is blocked by Gatekeeper and needs
  *System Settings → Privacy & Security → Open Anyway* (`xattr -dr com.apple.quarantine` also
  works). Only **Developer ID + notarisation** removes that friction.
- CI/CD: rcodesign runs on Linux runners too, and can notarise with an App Store Connect API key.

Details, evidence and the pitfalls we hit: [docs/CODE-SIGNING.md](docs/CODE-SIGNING.md).

## Tests

```bash
cd src-tauri && cargo test --lib      # unit tests (76)
./scripts/regression.sh               # gate: unit → file-replay → UI data chain → e2e → bundle+sign
```

Self-test modes (explicit env vars, never active on a normal launch):
`VOICEMAC_AUTOTEST=ui` (page-level data chain), `VOICEMAC_AUTOTEST=e2e` (real microphone),
`VOICEMAC_AUTOTEST_FILE=<wav>` (audio file replay, no microphone).

See [docs/TESTING.md](docs/TESTING.md) and [docs/TEST-MATRIX.md](docs/TEST-MATRIX.md).

## Runtime data

`~/Library/Application Support/io.github.yangyaofei.denpajack/`

The directory is configurable: set `data_dir` in the config file (or use the 数据目录 field on the
General page) to an absolute path, a `~/…` path, or a path relative to the default. Everything below
moves with it except `config.json`, which stays in the default location and acts as the anchor.

| File | Contents |
|---|---|
| `config.json` | all settings (profiles, dictionary, hotkeys, switches) |
| `app.log` | diagnostics (`[diag]` backend, `[ui]` frontend, device & key events) |
| `history.jsonl` | transcripts (raw / final / engine / delivery / audio path) |
| `recordings/` | retained audio, trimmed to the configured limit |
| `llm_logs/` | request and response body of every LLM call |

## Docs

| Document | Contents |
|---|---|
| [AGENTS.md](AGENTS.md) | project contract for contributors and agents: layout, commands, hard rules |
| [docs/SPEC.md](docs/SPEC.md) | requirements and architecture (features, modules, data flow) |
| [docs/CONTRACTS.md](docs/CONTRACTS.md) | cross-boundary contracts (threading, coordinates, lifecycle), enforced at build time |
| [docs/CODE-SIGNING.md](docs/CODE-SIGNING.md) | signing with rcodesign straight from a `.p12`, TCC requirements, measured evidence, pitfalls |
| [docs/CI-CD.md](docs/CI-CD.md) | GitHub Actions: test pipeline, release pipeline, repository credentials it needs |
| [docs/DIAGNOSTICS.md](docs/DIAGNOSTICS.md) | crash and silent-exit diagnostics: what the log covers, how to read it, self-test modes |
| [docs/DEPENDENCIES.md](docs/DEPENDENCIES.md) | dependency notes: the patched `handy-keys`, upstream PR, cleanup after merge |
| [docs/TESTING.md](docs/TESTING.md) / [docs/TEST-MATRIX.md](docs/TEST-MATRIX.md) | test organisation, "every fix adds a case" matrix |
| [docs/SYNC.md](docs/SYNC.md) | how front-end content stays in sync with back-end state |
| [docs/HANDY-COMPAT.md](docs/HANDY-COMPAT.md) | component-by-component comparison with Handy (overlay, tray, paste, hotkeys) |
| [docs/DEVIATION-AUDIT.md](docs/DEVIATION-AUDIT.md) | deliberate deviations from the reference implementation |
| [docs/FEATURE-BACKLOG.md](docs/FEATURE-BACKLOG.md) | open items and explicit non-goals (with reasons) |
| [docs/SPIKE-STREAMING.md](docs/SPIKE-STREAMING.md) | streaming ASR options (local and cloud, measured) |
| [docs/WINDOW-SEMANTICS.md](docs/WINDOW-SEMANTICS.md) | window levels and focus semantics |
| [docs/REFACTOR.md](docs/REFACTOR.md) | code-quality checklist (alignment with the Handy skeleton) |
| [docs/icon/README.md](docs/icon/README.md) | app icon: final assets, generation scripts, decision history |

## Known limits

- macOS only (the keyboard layer carries macOS-specific patches)
- Self-signed: fine on this machine, friction on other machines (see above)

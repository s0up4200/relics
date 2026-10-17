# Research: the Silo settings contract capabilities

Date: 2026-10-17. Status: research only. No production code changed. Issue: [#131](https://github.com/s0up4200/relics/issues/131).

Relics reads `GET /api/v2/settings/contract/capabilities` (`getSettingsContractCapabilities`) before the first effective read of a Profile. Today it uses only `manifest_revision`. This research explains `api_version`, `supports_batched_effective`, `state` and `allowed`. For each field, it gives the values, the meaning of each value, how the Silo web client gates on it, and what Relics must do.

Terms in this file:

- Capability document: the answer of a discovery operation. It tells a client if a feature exists on this server, for this caller.
- Gate: a check that a client does before it reads, shows or writes a setting.
- Gated key: a setting key that the web reads only after the gate passes.
- Base key: a setting key with `introduced_in: 1`. The web reads it without a gate.
- Fail closed: when a check cannot pass, the client does not use the feature.

## Sources

| Source | Version |
|---|---|
| silo-server checkout `~/github/oss/silo-server` | `b78c1ff`, 2026-10-11 |
| Settings contract | `contracts/settings/v1/manifest.json`, `api_version` 1, revision 17 |
| Relics | working tree of `feat/settings-capabilities` |

The checkout has no Go server code. It has only `contracts/`, `docs/` and `web/`. All server behavior in this file comes from the docs, the OpenAPI spec, the settings contract and the fixtures. The label "Not confirmed" marks a statement that no available source settles. Paths are in the silo-server repo, unless they start with `relics:`.

## Summary

- `api_version` is the version of the settings protocol. It changes only for a change that no revision rule can express (`docs/architecture/settings-contract.md:38-39`). The web requires `api_version === 1`. On a mismatch, it treats every gated key as unknown to the server (`web/src/hooks/queries/settingValues.ts:500`, `web/src/lib/settingsContract.ts:11`).
- `supports_batched_effective` tells if the effective resolver accepts a batch of keys (`contracts/api/v2/openapi.json:44009-44015`). When it is false, the web does not read the gated keys at all. The web has no read path with one key for each request. No source defines a non-batched effective read. Not confirmed: whether a server that sends `false` exists.
- `state` and `allowed` are the common head of every capability document (`docs/architecture/api-contract.md:919-925`). The web reads neither of them for settings. It reads them for other domains.
- When the capability read fails, the web reads and writes base keys as usual. It hides each gated key, or it shows the row with no value and no control.
- No source tells what the effective read or a write returns when `state` is not `available` or `allowed` is false. The settings operations declare `403` and `503`, but not `409` or `501` (`contracts/api/v2/openapi.json:189458-189750`, `:190765`).

## The capability document

- The answer has twelve required fields: nine settings fields plus the common `revision`, `state` and `allowed` (`docs/settings-api.md:67-70`, `contracts/api/v2/openapi.json:44024-44038`). The schema forbids other fields (`contracts/api/v2/openapi.json:43925`).
- The read is `profile_scoped`, with an optional `X-Profile-Id` (`contracts/api/v2/openapi.json:187747`, `:187927`). It can fail with `400`, `401`, `403`, `404`, `406`, `412`, `422`, `429`, `500` or `503` (`contracts/api/v2/openapi.json:187809-187916`).
- It supports `ETag` and `If-None-Match`. A match returns `304` with no body (`docs/settings-api.md:70-72`).
- The only committed fixtures are one `200` and one `401`. The `200` has `state: available`, `allowed: true` and both flags true (`contracts/api/v2/fixtures/index.json:8126-8162`, `contracts/api/v2/fixtures/get_settings_contract_capabilities_ok.json`). No fixture shows another `state`, `allowed: false` or a false flag.
- A client must read a capability document again after login, after a Profile change, when the app comes to the foreground, and after an invalidation event for the domain (`docs/architecture/api-contract.md:959-963`).

## `api_version`

- Values: an integer. The example and the manifest both have `1` (`contracts/api/v2/openapi.json:43931-43938`, `contracts/settings/v1/manifest.json:2`).
- Meaning: within one `api_version`, revisions are compatible in both directions. A client with a newer manifest hides what came after the server revision (`docs/architecture/settings-contract.md:42-45`). A change of value type, persistence class or meaning needs a new key, not a new `api_version` (`docs/architecture/settings-contract.md:48-54`). So a different `api_version` means that the rules of the protocol changed. The client cannot trust its own definitions.
- Web gate: `settingsCapabilitiesSupportKey` returns false when `api_version !== SETTINGS_API_VERSION` (`web/src/hooks/queries/settingValues.ts:495-504`). A test makes sure that `api_version: 2` fails the gate (`web/src/hooks/queries/settingValues.test.tsx:229-236`). The overlay preferences ignore `manifest_revision` on a mismatch (`web/src/hooks/useOverlayPrefs.ts:72-73`).
- On a mismatch, the web still reads and writes base keys, because those reads have no gate (`web/src/hooks/queries/qualityPreference.ts:28`, `web/src/hooks/queries/autoPlayNext.ts:40`, `web/src/pages/settings/PlaybackSettings.tsx:448-464`).

## `manifest_revision`

- Values: an integer, the revision of the server manifest (`contracts/api/v2/openapi.json:43967-43974`). It replaces the old numeric `revision` (`docs/settings-api.md:68-69`, `docs/update-to-1.0.md:191`).
- Meaning: a client hides the definitions and features that came after that revision (`docs/settings-api.md:691-693`). Each key, scope, enum member and bound has its own `introduced_in` (`docs/architecture/settings-contract.md:50`, `:58-61`). For example, the audio-language option `zh-Hans` has `introduced_in: 8` (`contracts/settings/v1/manifest.json:12`).
- Effect: a key that the server does not know makes the whole effective read fail with `422 validation_failed` at `query.keys` (`contracts/api/v2/fixtures/list_effective_settings_unknown_key.json`, `contracts/api/v2/fixtures/index.json:9459-9468`).
- Web gate: `manifest_revision >= introducedIn` for each gated key (`web/src/hooks/queries/settingValues.ts:501`). The web gates keys, not enum members.

## `supports_batched_effective`

- Values: `true` or `false`. Meaning: "Whether the effective resolver accepts a batch of keys" (`contracts/api/v2/openapi.json:44009-44015`).
- The docs of the v1 bridge say that revision-5 customization requires the flag, and that "Any missing flag fails closed" (`docs/settings-api.md:695-698`).
- Web gate: the flag must be exactly `true`. A false or absent flag fails the gate for every gated key (`web/src/hooks/queries/settingValues.ts:502`, `web/src/hooks/queries/settingValues.test.tsx:246-265`).
- When the flag is false, the web does not read the gated keys. It does not change to one request for each key. Examples:
  - Interface Settings shows "Server upgrade required" and no controls (`web/src/pages/settings/InterfaceSettings.tsx:277-288`).
  - Title art and the device settings hide their rows (`web/src/pages/settings/TitleArtSettingsGroup.tsx:23-25`, `web/src/pages/settings/DeviceSettings.tsx:264-271`).
  - Playback Settings leaves the gated keys out of the read. It shows the legacy intro switch in place of `playback.intro_skip_mode` (`web/src/pages/settings/PlaybackSettings.tsx:443-458`).
- The web reads base keys in one batched `GET settings/values/effective?keys=a&keys=b` and does not look at the flag (`web/src/pages/settings/PlaybackSettings.tsx:448-464`, `web/src/hooks/queries/settingValues.ts:196-217`).
- No source tells how to read without a batch. The plain fallback is one key for each request (`?keys=a`). Not confirmed: whether a server with `false` accepts it. `getSettingValue` reads a stored value at one scope. It does not resolve the effective value (`docs/settings-api.md:45`, `:125-126`).

## `state`

- Values: `available`, `disabled`, `not_configured` or `unsupported` (`contracts/api/v2/openapi.json:43992-44003`). The description says "Support and configuration state, not health".
- The common meaning for every capability document (`docs/architecture/api-contract.md:919-925`, `:938-944`, `:955-956`):
  - `available`: the server supports the feature, and the feature has its configuration.
  - `disabled`: an administrator turned the feature off. Its operations return `409 capability_disabled`.
  - `not_configured`: required configuration is missing. Its operations return `409 capability_not_configured`. Missing service wiring gives a `not_configured` document.
  - `unsupported`: this server build cannot do it. Its operations return `501 capability_unsupported`.
  - An unknown value: the server can add new states. A client treats an unknown state as unavailable (`docs/architecture/api-contract.md:925`, `docs/update-to-1.0.md:188`).
  - A temporary failure is not a state. It returns a `503` problem, with `Retry-After` when it applies.
- When the server wiring lacks a gate, its operations fail closed with `503 dependency_unavailable`. They stay in the route table (`docs/architecture/api-contract.md:345-347`).
- Settings: the settings operations declare `403` and `503`, but no `409` and no `501` (`contracts/api/v2/openapi.json:189458-189750`, `:190765`). So when settings are not `available`, the likely answer of an effective read or a write is `503 dependency_unavailable`. Not confirmed: the real server code, and whether the server ever sends a `state` other than `available` for settings.
- Web: the settings code never reads `state` (`web/src/hooks/queries/settingValues.ts:463-472`). Other domains gate on `state === "available"`, for example metadata AI and ratings (`web/src/hooks/queries/metadataAI.ts:27`, `web/src/hooks/queries/ratingsCapability.ts:17`).

## `allowed`

- Values: `true` or `false`. Meaning: "Whether the current principal may use the capability" (`contracts/api/v2/openapi.json:43927-43930`). A scoped capability document always has it (`docs/architecture/api-contract.md:932`).
- `allowed` can be false while `state` is `available` (`docs/architecture/api-contract.md:956-958`). A visible denial on an operation is `403`. A hidden feature is `404` (`docs/architecture/api-contract.md:941-942`). Discovery never grants authority. Each operation checks authorization again (`docs/architecture/api-contract.md:946-949`).
- Settings: no source tells when `allowed` is false for settings. Not confirmed: what the effective read and the writes return then. `403` is the likely answer.
- Web: the settings code never reads `allowed`. Other domains gate on it, for example theme music and title requests (`web/src/pages/ItemDetail/useThemeMusic.ts:74-75`, `web/src/hooks/useCanRequest.ts:26`).

## When the capability read fails

- `settingsCapabilitiesSupportKey(undefined, …)` is false. So a failed read and an old server give the same gate answer (`web/src/hooks/queries/settingValues.test.tsx:229`). A page that must tell them apart uses the state of the query (`web/src/playback/WatchPlaybackChrome.tsx:582-590`).
- The web does not read gated keys:
  - Interface Settings shows "Customization unavailable" and disables editing "to protect your existing choices" (`web/src/contexts/UICustomizationProvider.tsx:31-34`, `web/src/pages/settings/InterfaceSettings.tsx:263-275`).
  - Playback Settings keeps the intro row, with no value and no control (`web/src/pages/settings/PlaybackSettings.tsx:443-447`, `:642-648`). It does not show the legacy switch, because a write of the old key can change a stored `never` (`web/src/pages/settings/PlaybackSettings.tsx:432-442`).
- The web reads and writes base keys as usual. No write hook looks at the capabilities (`web/src/hooks/queries/settingValues.ts:351-381`).
- The capability query keeps an answer for 30 minutes (`web/src/hooks/queries/settingValues.ts:519-535`).

## What this means for Relics

Relics has eight keys. `playback.intro_skip_mode` has `introduced_in: 7`. The other seven have `introduced_in: 1` (`relics:src/settings.rs:112-160`, `contracts/settings/v1/manifest.json:529`).

1. If `api_version` is 1, read normally.
2. If `api_version` is not 1, treat it like a failed capability read: show rows and refuse writes. Reason: the meaning of the keys can be different (`docs/architecture/settings-contract.md:38-39`). The web is less strict, because it still reads and writes base keys. Relics refuses writes, so that it does not store a value whose meaning it does not know.
3. For `manifest_revision`, keep the current gate. Leave out each key with `introduced_in` above `manifest_revision`, so that the batch does not fail with `422`. Also hide each choice whose `introduced_in` is above it, for example a newer language option. Relics does not do this today.
4. If `supports_batched_effective` is true, read normally in one batch.
5. If `supports_batched_effective` is false, read without batch: send one `GET settings/values/effective?keys=<key>` for each key. Reason: Relics has only eight keys, and this sends no batch. Not confirmed: whether the server accepts it. If the read for one key fails, show that row with no value and refuse its write. The web hides gated keys in this case and keeps one batch for base keys, so the web gives no example to copy.
6. If `state` is `available`, read normally.
7. If `state` is `disabled`, `not_configured`, `unsupported` or an unknown value, show rows, refuse writes, and do not send the effective read. Reason: the common contract says that the operations fail (`409`, `501` or `503`), and an unknown state is unavailable (`docs/architecture/api-contract.md:925`). Show a message that settings are off on this Server, not that Relics cannot reach the Server. The web ignores `state` for settings, so this is stricter than the web.
8. If `allowed` is true, read normally.
9. If `allowed` is false, show rows, refuse writes, and do not send the effective read. Reason: each operation checks authority again, and the likely answer is `403`. Show a message that this Profile cannot change settings. Not confirmed: when the server sends `false` for settings.
10. If the capability read fails (network error, `5xx`, `401`), keep the current behavior. Show rows with the current values, and refuse each write with "Relics cannot reach the settings of the Server". This is stricter than the web, which still writes base keys, but it is safe. Read the capabilities again on the next load and after a Profile change.
11. Ignore `supports_idempotent_writes`, `supports_atomic_shortcuts`, `scopes`, `client_families`, `definition_count` and `contract_etag`. Relics writes plain values at the `profile` scope, and no v2 write carries a mutation id (`docs/settings-api.md:103-107`).
12. If an effective read or a write answers `503 dependency_unavailable`, `403` or `409`, treat it like item 7 or item 9 for that load. Read the capabilities again on the next load, because the document can change (`docs/architecture/api-contract.md:959-963`).

## Decisions for #131

The user chose these points on 2026-10-17. Where they do not agree with items 3, 5, 7, 8 and 9 above, they replace them:

- `api_version` is not 1: Relics does the same as for a failed capability read (item 2).
- `supports_batched_effective` is false: Relics does the same as the web. It reads only the keys of revision 1, in one batch, and hides the other rows. It does not read one key for each request.
- `state` is not `available`, or `allowed` is false: Relics does the same as for a failed capability read, with the same message.
- Choices with an `introduced_in` above `manifest_revision`: not part of #131.

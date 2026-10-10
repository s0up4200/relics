# Research: generate the Silo API types, or write them by hand?

Ticket: issue #5. Map: issue #1.

## Answer

Use a hybrid of generated and hand-written code:

- Generate the wire types (the request and response structs) from the Silo
  OpenAPI file with typify 0.8.0. Generate types only. Do not generate an
  HTTP client.
- Before typify reads the file, a small script cuts the spec down to the
  schemas that the first version uses. The script also removes
  `additionalProperties: false` and `enum`, and it fixes one name clash.
  Then the types accept new fields and new enum values from the server.
- Write by hand the parts that the spec does not describe well. These are the
  open enums with an `Unknown(String)` fallback, the playback decision union,
  the error type, and the domain types of the app.
- Copy the contract fixtures of the server into the repo. A test decodes each
  fixture into its generated type. This test finds drift between the client
  and the server.

Do not use progenitor or openapi-generator. Progenitor refuses OpenAPI 3.1.
Both tools also choose the HTTP client for you, and the HTTP client is a
separate ticket.

The rest of this file gives the facts behind this answer.

## Sources and pinned commits

| Short name | Repository | Commit or version |
| --- | --- | --- |
| `SS` | [Silo-Server/silo-server](https://github.com/Silo-Server/silo-server) | `ec1dcb67c77f59688692574516276e8f6c72dfeb` (2026-10-10) |
| `SA` | [Silo-Server/silo-apple](https://github.com/Silo-Server/silo-apple) | `5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba` |
| `SAN` | [Silo-Server/silo-android](https://github.com/Silo-Server/silo-android) | `e535ddceb5bedd22b75d0e53abca27b3640982df` |
| `PG` | [oxidecomputer/progenitor](https://github.com/oxidecomputer/progenitor) | tag `v0.15.0`, commit `76a3b70f0eb11e044b52ee15bd07bcc370c8adb5` |
| `TY` | [oxidecomputer/typify](https://github.com/oxidecomputer/typify) | tag `v0.8.0`, commit `03e51a2785d6f4dc1bbc1b949f9eb99fda5e91a1` |

Crate versions and release dates come from the crates.io API on 2026-10-10.

## 1. Where openapi.json comes from

Huma generates the file from Go code. Nobody edits it by hand.

- The v2 API uses Huma v2 on the chi router. The contract doc says: "Go input
  and output types are the source of truth. Huma derives OpenAPI 3.1 and JSON
  Schema from those types"
  ([`SS/docs/architecture/api-contract.md#L28-L32`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/architecture/api-contract.md#L28-L32)).
- `contracts/api/v2/openapi.json` "is a deterministic generated artifact. It
  is never edited by hand." The server sends the exact committed bytes at
  `/api/v2/openapi.json`
  ([`SS/docs/architecture/api-contract.md#L58-L75`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/architecture/api-contract.md#L58-L75),
  [`SS/internal/apiv2/openapi.go#L56-L66`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/apiv2/openapi.go#L56-L66)).
- `make apiv2-openapi` runs `cmd/apiv2-openapi`. This command builds the
  document from the Go registries. It opens no database or network and reads
  no environment
  ([`SS/Makefile#L256-L262`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/Makefile#L256-L262),
  [`SS/cmd/apiv2-openapi/main.go#L1-L8`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/cmd/apiv2-openapi/main.go#L1-L8)).

CI keeps the file in sync with the code in three ways:

1. `verify-apiv2-openapi` generates the file again and compares it byte for
   byte with the committed file. A stale file fails the job
   ([`SS/Makefile#L264-L273`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/Makefile#L264-L273)).
   The Go lint job runs it in the `contracts` lane
   ([`SS/.github/actions/ci-tools/go-checks.mk#L16-L20`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/.github/actions/ci-tools/go-checks.mk#L16-L20),
   [`SS/.github/workflows/trusted-linux-ci.yml#L341`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/.github/workflows/trusted-linux-ci.yml#L341)).
2. `verify-apiv2-contract` runs a semantic diff (oasdiff) against the base
   branch. A breaking change fails unless `breaking-approvals.json` approves
   it. After a `LOCKED` file exists, no approval applies
   ([`SS/Makefile#L294-L312`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/Makefile#L294-L312)).
   At this commit there is no `contracts/api/v2/LOCKED` file. Silo is
   pre-1.0, so approved breaking changes can still land.
3. `verify-apiv2-fixtures` makes the fixtures through the real v2 router. Then
   it validates every body against the OpenAPI schema that its index entry
   names
   ([`SS/Makefile#L314-L327`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/Makefile#L314-L327)).
   The server does not validate responses at run time. The fixtures are the
   only response check
   ([`SS/docs/architecture/api-contract.md#L86-L90`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/architecture/api-contract.md#L86-L90)).

Each release also publishes `openapi.json` and its SHA-256 as release assets
([`SS/.github/workflows/release.yml#L67-L79`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/.github/workflows/release.yml#L67-L79)).
The web client generates its TypeScript types from the same file with
`openapi-typescript`, and CI compares them too
([`SS/Makefile#L275-L292`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/Makefile#L275-L292)).

The official native clients write their models by hand. Apple writes Swift
enums with a `case unknown(String)` fallback
([`SA/iosApp/iosApp/Networking/APIv2/APIv2Models.swift#L60-L72`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/iosApp/iosApp/Networking/APIv2/APIv2Models.swift#L60-L72)).
Apple also keeps its retry list "by hand in step with the contract's
`x-silo-retry-safety` annotations"
([`SA/docs/native-api-v2.md#L68-L70`](https://github.com/Silo-Server/silo-apple/blob/5c1dcfb0c71dcf3d4ab75eb4ba1f813988c8b2ba/docs/native-api-v2.md#L68-L70)).
The server Makefile copies the fixtures into the Apple and Android test trees
([`SS/Makefile#L330-L349`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/Makefile#L330-L349)).
Inference: Android writes its models by hand too, because its Gradle files
have no OpenAPI plugin.

## 2. The shape of the spec

These counts come from `SS/contracts/api/v2/openapi.json`:

- OpenAPI `3.1.0`, 685 paths, 852 operations and 1152 component schemas.
- Nullable fields use 3.1 type arrays, for example `["string", "null"]`, in
  251 places. The file does not use the 3.0 `nullable` keyword.
- `oneOf` occurs 3 times. `anyOf`, `allOf`, `discriminator` and `const` do
  not occur.
- 1214 schemas have `additionalProperties: false`. This includes response
  schemas.
- Errors are `application/problem+json` with the `Problem` schema
  ([`SS/contracts/api/v2/openapi.json#L39434-L39490`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/contracts/api/v2/openapi.json#L39434-L39490)).
- Operations carry vendor extensions such as `x-silo-retry-safety`. The file
  has 2821 `x-silo-*` keys.

The first version uses about 29 operations. They are login, refresh, logout,
device start, poll and cancel, list profiles, verify PIN, home layout, home
sections, section items, user libraries, catalog list and query, catalog item,
seasons, episodes, watch detail, image capabilities, playback capabilities,
start, replan, progress, stop, list progress, file and item markers, system
info and server identity.

These operations reach 134 schemas. In that subset there is no `oneOf`, no
untyped map, and only two nullable type arrays. 20 properties have an `enum`,
and 482 properties are plain strings. The 3 `oneOf` uses are in progress sync
and server connections
([`#L40607`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/contracts/api/v2/openapi.json#L40607),
[`#L43252`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/contracts/api/v2/openapi.json#L43252)),
which the first version does not use.

Inference: for the first version, the difficult 3.1 features do not matter.
The real problems are lost enums, unions that the spec does not express, and
strict objects. Section 3 lists them.

## 3. Gaps and mismatches for the first-version endpoints

### 3.1 Playback v3 loses its enums and its union

`PlaybackDecision` has `outcome` as a plain string. `playback_plan`,
`session_id` and `terminal` are all optional
([`SS/contracts/api/v2/openapi.json#L37417`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/contracts/api/v2/openapi.json#L37417)).
The v3 JSON Schema says more. There, `outcome` is `playable` or
`adaptation_unavailable`. A `oneOf` makes `playable` carry `session_id` and
`playback_plan`, and makes `adaptation_unavailable` carry `terminal`
([`SS/docs/design/schemas/playback-v3/v3/decision-response.schema.json#L24-L80`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/design/schemas/playback-v3/v3/decision-response.schema.json#L24-L80),
[`SS/docs/architecture/playback-protocol-v3.md#L161`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/architecture/playback-protocol-v3.md#L161)).

The v3 decision schema has 12 enum properties. The OpenAPI decision schemas
keep none of them. Examples are `plan.delivery`, `stream.protocol`,
`stream.header_refresh`, `timeline.seek_restoration` and
`subtitle_decision.mode`. In the OpenAPI file each one is a plain string
([`SS/contracts/api/v2/openapi.json#L37481`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/contracts/api/v2/openapi.json#L37481),
[`#L44503`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/contracts/api/v2/openapi.json#L44503)).
The start request loses 4 of its 5 enums in the same way. All playback
schemas in the OpenAPI file together have only 6 enum properties.

The cause is in the Go code. The fields use named string types such as
`DecisionOutcomeV3` and `DeliveryV3` without an `enum` tag
([`SS/internal/apiv2/playback.go#L120-L127`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/apiv2/playback.go#L120-L127),
[`SS/internal/playback/protocol_v3.go#L191-L201`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/playback/protocol_v3.go#L191-L201)).
The one field with an explicit `enum:"server,client"` tag,
`progress_persistence`, keeps its enum
([`SS/internal/apiv2/playback.go#L59`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/internal/apiv2/playback.go#L59)).
The contract has a rule against this: "Polymorphic objects use a required
string discriminator and named schemas"
([`SS/docs/architecture/api-contract.md#L694-L699`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/architecture/api-contract.md#L694-L699)).

A generated `PlaybackDecision` is thus a struct of optional fields. The
player branches on `outcome` and `delivery`, so these types need a
hand-written layer in any case.

### 3.2 The v3 JSON Schemas describe the v1 envelope, not v2

The v3 spec says that a client can implement playback from the spec and the
schemas alone. But the schema files do not match the v2 wire format:

- Their descriptions name `/api/v1/playback/start` and other v1 paths
  ([`SS/docs/design/schemas/playback-v3/v3/decision-response.schema.json#L5`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/design/schemas/playback-v3/v3/decision-response.schema.json#L5),
  [`start-request.schema.json#L5`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/design/schemas/playback-v3/v3/start-request.schema.json#L5)).
- `file_id` is an integer
  ([`start-request.schema.json#L24-L27`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/design/schemas/playback-v3/v3/start-request.schema.json#L24-L27)),
  and the valid fixtures have `"requested_media_file_id": 42`. On v2 every id
  is an opaque string
  ([`SS/docs/architecture/api-contract.md#L680-L683`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/architecture/api-contract.md#L680-L683),
  [`SS/docs/playback-api.md#L95-L96`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/playback-api.md#L95-L96)).
- The v2 start body adds `installation_id`. It also requires
  `quality_preference`, `metered` and `client_features`
  ([`SS/contracts/api/v2/openapi.json#L38573`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/contracts/api/v2/openapi.json#L38573)).
  The v3 schema does not require them
  ([`start-request.schema.json#L7-L15`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/design/schemas/playback-v3/v3/start-request.schema.json#L7-L15)).

Do not generate v2 types from these files. Use them only as the source for
enum values and protocol rules.

### 3.3 Strict objects against additive responses

Almost every schema has `additionalProperties: false`, response schemas too.
But the contract says that first-party clients "decode leniently: clients
ignore unknown response fields and preserve an unknown-enum fallback"
([`SS/docs/architecture/api-contract.md#L292-L293`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/architecture/api-contract.md#L292-L293)).
It also says that "response enums are additive"
([`#L694-L696`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/architecture/api-contract.md#L694-L696)).
The v3 README agrees: "changes are additive only: new optional fields, and
new enum values"
([`SS/docs/design/schemas/playback-v3/README.md#L1-L8`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/design/schemas/playback-v3/README.md#L1-L8)).

A generator that obeys the schema thus makes a client that fails when the
server adds a field or an enum value. Section 4.2 shows this with typify.

### 3.4 Smaller gaps

1. Some unions exist only in prose. `DeviceLoginPoll.tokens` is present "only
   when status is approved", but the schema marks it optional with no union
   ([`SS/contracts/api/v2/openapi.json#L26050`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/contracts/api/v2/openapi.json#L26050)).
   An `Option` field is enough here.
2. `MarkerUpdate` has three states per field: "omitted leaves unchanged; null
   clears"
   ([`SS/contracts/api/v2/openapi.json#L33253`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/contracts/api/v2/openapi.json#L33253),
   [`SS/docs/markers-api.md#L137-L138`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/markers-api.md#L137-L138)).
   A plain `Option<T>` cannot tell "omitted" from "null". The first version
   only reads markers, so this does not matter yet.
3. The v2 fixtures cover login, refresh, device start, device poll (with an
   `approved` poll), profiles, PIN, home, catalog, watch detail, markers,
   progress, system info, playback capabilities, a playable start, a seek
   replan and stop. No v2 fixture covers an `adaptation_unavailable` decision
   or a failure-recovery replan (source: `SS/contracts/api/v2/fixtures/index.json`).
   The client must write its own fixtures for those cases.
4. Retry safety exists only in vendor extensions. `x-silo-retry-safety` is on
   the operations, for example
   [`SS/contracts/api/v2/openapi.json#L51325`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/contracts/api/v2/openapi.json#L51325).
   No generator in section 4 reads it. The transport layer must encode it by
   hand, as Apple does.
5. The auth doc calls the device `user_code` "eight digits". Its example and
   the fixture are both `4821-7730`
   ([`SS/docs/auth-api.md#L377`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/docs/auth-api.md#L377)).
   The wire value has a hyphen. Show the string as it comes, and do not
   reject it for having a hyphen.
6. `stopPlayback` is a `DELETE` with a JSON body (`PlaybackStopBody`).
   Inference: HTTP allows this, but the chosen HTTP client must send a body
   on `DELETE`.

Many parts match. For the auth, profile, PIN, markers and system info
schemas, the field names, required sets and enums agree with the docs.
`TokenPair` and `RefreshedTokens` match the login and refresh shapes in
`docs/auth-api.md`. `DeviceLoginPoll.status` has the full enum.
`ProfileVerification` has `valid` and a nullable `expires_at`. The text about
string ids in `playback-api.md` matches the schema.

## 4. Rust generators against this spec

### 4.1 Candidates

| Tool | Version that passes the 7-day rule | OpenAPI 3.1 | Types only | HTTP client |
| --- | --- | --- | --- | --- |
| progenitor | 0.15.0 (2026-09-10). 0.15.1 is 1 day old. | No. It refuses any version that does not start with `3.0.` | No, full client | reqwest |
| openapi-generator `rust` | Java tool | Not stated | No models-only option | reqwest (default) or hyper |
| typify | 0.8.0 (2026-09-09). 0.10.0-alpha.2 is 5 days old. | Reads JSON Schema, accepts 3.1 type arrays | Yes | none |
| oas3-gen | 0.28.0 (2026-08-23) | Yes, 3.1 | Yes, `types` mode is the default | not documented |
| openapi-to-rust | 0.19.0 (2026-09-26). 4 releases in 2 weeks. | Yes, 3.0 and 3.1 | Yes, `--types-only` | reqwest |

Sources for the table:

- progenitor. The README says that it targets "the OpenAPI 3.0.x
  specification"
  ([`PG/README.md#L1-L5`](https://github.com/oxidecomputer/progenitor/blob/76a3b70f0eb11e044b52ee15bd07bcc370c8adb5/README.md#L1-L5)).
  The code refuses `3.1.0` with the comment "progenitor currenlty only
  support OAS 3.0.x"
  ([`PG/progenitor-impl/src/lib.rs#L702-L712`](https://github.com/oxidecomputer/progenitor/blob/76a3b70f0eb11e044b52ee15bd07bcc370c8adb5/progenitor-impl/src/lib.rs#L702-L712)).
  It parses with the `openapiv3` crate and depends on reqwest
  ([`PG/Cargo.toml#L42-L49`](https://github.com/oxidecomputer/progenitor/blob/76a3b70f0eb11e044b52ee15bd07bcc370c8adb5/Cargo.toml#L42-L49)).
  Progenitor uses typify for its types, so it adds only a reqwest client on
  top of typify.
- openapi-generator `rust`. The
  [generator docs](https://openapi-generator.tech/docs/generators/rust/)
  list the `library` values `hyper`, `hyper0x`, `reqwest` and
  `reqwest-trait`, with `reqwest` as the default. The feature table marks
  `oneOf` as supported. It marks `anyOf`, `allOf`, `Polymorphism`, `Union`
  and `Null` as not supported. The help text calls the generator "beta". It
  needs a Java runtime.
- typify. See section 4.2.
- oas3-gen. The [docs.rs page](https://docs.rs/crate/oas3-gen/latest) says
  that it generates types from OpenAPI 3.1.x, with the modes `types`,
  `client`, `client-mod` and `server-mod`. The page does not say how it
  treats `additionalProperties: false` or unknown enum values. The crate has
  about 30,000 downloads in total.
- openapi-to-rust. The [docs.rs page](https://docs.rs/crate/openapi-to-rust/latest)
  says that it reads 3.1 type arrays, makes tagged enums from
  discriminators, and has a `--types-only` flag. Generated clients use
  reqwest. Open enums need an `[extensible_enums]` setting. Versions 0.19.0
  to 0.22.0 came out between 2026-09-26 and 2026-10-06.

### 4.2 A typify run against the Silo spec

I ran typify on the spec in a scratch directory:

1. I installed `cargo-typify@0.8.0` with `--locked`. The lock file in that
   release is from 2026-09-09, so every dependency is older than 7 days.
2. A Python script took the 134 schemas that the first-version operations
   reach. It wrote them into one JSON Schema file under `$defs`.
3. typify made 181 structs and 20 enums, about 260 KB of Rust.
4. A test crate decoded every v2 fixture for those operations into the
   generated type that the fixture index names.

The results:

1. The first build failed on a name clash. The inline enum
   `CatalogQuery.group` and the schema `CatalogQueryGroup` both become
   `CatalogQueryGroup` in Rust
   ([`SS/contracts/api/v2/openapi.json#L22632-L22636`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/contracts/api/v2/openapi.json#L22632-L22636),
   [`#L22744`](https://github.com/Silo-Server/silo-server/blob/ec1dcb67c77f59688692574516276e8f6c72dfeb/contracts/api/v2/openapi.json#L22744)).
   A rename of the schema in the script fixed it.
2. After the rename, all 74 fixtures decoded without error.
3. `["string", "null"]` with `format: date-time` became
   `Option<chrono::DateTime<Utc>>`.
4. Every struct got `#[serde(deny_unknown_fields)]` (134 of 134). This
   follows the typify rule that `additionalProperties: false` maps to
   `deny_unknown_fields`
   ([`TY/README.md#L103-L108`](https://github.com/oxidecomputer/typify/blob/03e51a2785d6f4dc1bbc1b949f9eb99fda5e91a1/README.md#L103-L108),
   [`TY/typify-impl/src/structs.rs#L80-L83`](https://github.com/oxidecomputer/typify/blob/03e51a2785d6f4dc1bbc1b949f9eb99fda5e91a1/typify-impl/src/structs.rs#L80-L83)).
   I added one unknown field to the `get_system_info_ok` fixture, and
   decoding failed with "unknown field `new_server_field`".
5. `DeviceLoginPollStatus` became a closed Rust enum. The value `"paused"`
   failed with "unknown variant `paused`".
6. Then I ran the script again and removed `additionalProperties: false` and
   every `enum`. No struct had `deny_unknown_fields`, and all 74 fixtures
   still decoded. The unknown field and the unknown enum value both decoded
   too.

typify writes plain serde types with no HTTP code. The generated code needs
`serde`, `serde_json`, `chrono` and `regress`. typify uses the `schemars` 0.8
schema model
([`TY/Cargo.toml#L37`](https://github.com/oxidecomputer/typify/blob/03e51a2785d6f4dc1bbc1b949f9eb99fda5e91a1/Cargo.toml#L37)).
It does not claim full JSON Schema 2020-12 support, but this subset did not
need it.

## 5. Options compared

### A. Generate a full client with progenitor or openapi-generator

Progenitor cannot read the file without a step that converts 3.1 to 3.0.
Both tools choose the HTTP client. Neither tool reads `x-silo-retry-safety`.
Reject this option.

### B. Write every type by hand

Apple and Android do this. It gives full control over open enums and unions.
But the subset has 134 schemas and about 500 fields. Silo is pre-1.0, and its
schemas still change. Inference: without a generator, the hand-written types
drift from the server.

### C. Use typify for wire types, with a hand-written layer on top

typify reads this subset after the script makes the input lenient. The
fixtures prove the result. The hand-written layer is small, because only a
few fields control the behavior of the app:

- `PlaybackDecision` as a Rust enum, `Playable { session_id, plan }` or
  `Unavailable { terminal }`, with a fallback for an unknown `outcome`.
- Open enums with `Unknown(String)` for `delivery`, `stream.protocol`,
  `header_refresh`, `seek_restoration`, `DeviceLoginPoll.status` and the
  marker `kind`.
- The `Problem` error, keyed by the last segment of `type`.
- The domain types (Server, Account, Profile, Item, Playback session) from
  the glossary ticket.

## 6. Recommendation

Choose option C:

1. Pin a Silo server commit. Copy `contracts/api/v2/openapi.json` and the
   fixtures for the first-version operations into the repo.
2. Add one small generator script, as a Rust `xtask` or a short script. The
   script takes the list of operation IDs that the client uses and collects
   the schemas that they reach. It removes `additionalProperties: false`,
   `enum`, `examples` and the non-standard `patternDescription`, and it
   renames `CatalogQueryGroup`. Then it runs typify 0.8.0 and writes one
   committed `.rs` file.
3. Run the script by hand when the pinned server commit changes. Do not run
   it in `build.rs`. A committed file is easy to review and keeps builds
   fast.
4. Put the generated types in their own crate or module with no HTTP code.
   Then the transport ticket can choose any HTTP client.
5. Write the hand-written layer from option C. Take the enum values from the
   v3 JSON Schemas and the protocol doc, but keep every enum open.
6. Add a test that decodes every copied fixture into its generated type. Add
   a test that an unknown field and an unknown enum value still decode. Add
   hand-made fixtures for `adaptation_unavailable` and for a
   failure-recovery replan.
7. Optionally, report the spec gaps upstream. The playback v3 enums and the
   decision union are missing from `openapi.json`. Responses carry
   `additionalProperties: false`, but the contract tells clients to ignore
   unknown fields. If Silo fixes these, the script can keep request enums.

typify 0.10.0 is in alpha now. Before you move to it, test again how it
treats `additionalProperties` and enums. If typify stops working for this
spec, try oas3-gen next.

## Open questions

- Must the client log a warning when `contract_digest` from
  `GET /api/v2/system/info` differs from the pinned spec? The spec calls the
  field "Diagnostic and cache-identity only: never feature-detect on it", so
  a log line is the most it can do.
- Who reports the spec gaps upstream? Will the Silo team add enum tags to the
  playback v3 Go types before 1.0?

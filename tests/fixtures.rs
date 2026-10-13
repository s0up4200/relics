//! Fixture drift: each server fixture that the app uses decodes into its type, also
//! when a newer server adds a field or an enum value. scripts/refresh-fixtures.py
//! copies the fixtures from the server commit in tests/fixtures/silo/SERVER_COMMIT.

use relics::api::{Problem, types, wire};
use serde_json::{Value, json};

/// `silo` holds the server fixtures, `own` the fixtures that this app adds in the same format.
fn read_in(dir: &str, file: &str) -> Value {
    let path = format!("{}/tests/fixtures/{dir}/{file}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn read(file: &str) -> Value {
    read_in("silo", file)
}

/// Decodes `body` as the schema `name`. `None` means that the test knows no type for it.
fn decode(name: &str, body: Value) -> Option<Result<(), serde_json::Error>> {
    macro_rules! wire_types {
        ($($ty:ident),+ $(,)?) => {
            match name {
                "Problem" => Some(serde_json::from_value::<Problem>(body).map(drop)),
                $(stringify!($ty) => Some(serde_json::from_value::<wire::$ty>(body).map(drop)),)+
                _ => None,
            }
        };
    }
    wire_types!(
        Account,
        AuthProviderCollection,
        CatalogBrowseCollection,
        CatalogFilters,
        CatalogItemCollection,
        CatalogItemDetail,
        CollectionPersonalCollectionItem,
        CollectionUserLibrary,
        DeviceLoginCancel,
        DeviceLoginCapability,
        DeviceLoginPoll,
        DeviceLoginStart,
        EffectiveSettingCollection,
        EpisodeCollection,
        FeatureStatus,
        FileMarkers,
        FileVersionCollection,
        LibraryCollectionTab,
        MediaRequest,
        MediaRequestCollection,
        OAuthCompletion,
        OAuthHandshakeCapabilitiesOutputBody,
        Person,
        PersonCollection,
        PersonalCollectionCollection,
        PlaybackCapabilities,
        PlaybackDecision,
        PlaybackMutation,
        ProfileCollection,
        ProfileVerification,
        ProgressSyncBatchResult,
        RefreshedTokens,
        SeasonCollection,
        Section,
        SectionCollection,
        SectionLayout,
        ServerIdentity,
        SettingValue,
        SystemInfo,
        TokenPair,
        WatchTrickplay,
    )
}

/// Each fixture with a JSON body, as (fixture name, schema name, body).
fn fixtures() -> Vec<(String, String, Value)> {
    let server = read("index.json")["fixtures"].as_array().unwrap().len();
    assert!(server > 50, "the index lists only {server} fixtures");
    ["silo", "own"]
        .into_iter()
        .flat_map(|dir| {
            let index = read_in(dir, "index.json");
            index["fixtures"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|f| {
                    let schema = f["schema"].as_str()?.strip_prefix("#/components/schemas/").unwrap();
                    let body = read_in(dir, f["body_file"].as_str()?);
                    Some((f["name"].as_str().unwrap().to_string(), schema.to_string(), body))
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Adds a field that no server sends to each object in `value`. The value is a string,
/// because some objects are maps of strings, such as headers.
fn add_unknown_fields(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.values_mut().for_each(add_unknown_fields);
            map.insert("field_from_a_newer_server".into(), json!("x"));
        }
        Value::Array(items) => items.iter_mut().for_each(add_unknown_fields),
        _ => {}
    }
}

#[test]
fn each_fixture_decodes_into_its_type() {
    for (name, schema, body) in fixtures() {
        match decode(&schema, body) {
            Some(Ok(())) => {}
            Some(Err(err)) => panic!("{name}: {schema}: {err}"),
            None => panic!("{name}: no type for schema {schema}; add it to decode()"),
        }
    }
}

#[test]
fn each_fixture_decodes_with_unknown_fields() {
    for (name, schema, mut body) in fixtures() {
        add_unknown_fields(&mut body);
        if let Some(Err(err)) = decode(&schema, body) {
            panic!("{name}: {schema}: {err}");
        }
    }
}

#[test]
fn an_unknown_enum_value_decodes() {
    let mut body = read("playback_start_opaque_ids.json");
    body["playback_plan"]["delivery"] = json!("delivery_from_a_newer_server");
    body["playback_plan"]["stream"]["protocol"] = json!("hls");
    let decision: wire::PlaybackDecision = serde_json::from_value(body).unwrap();
    let plan = decision.playback_plan.unwrap();
    assert_eq!(types::Delivery::from(plan.delivery), types::Delivery::Unknown("delivery_from_a_newer_server".into()));
    assert_eq!(types::StreamProtocol::from(plan.stream.protocol), types::StreamProtocol::Hls);
}

#[test]
fn an_open_enum_keeps_an_unknown_value_through_serde() {
    let kind: types::MarkerKind = serde_json::from_value(json!("outro")).unwrap();
    assert_eq!(kind, types::MarkerKind::Unknown("outro".into()));
    assert_eq!(serde_json::to_value(kind).unwrap(), json!("outro"));
    let kind: types::MarkerKind = serde_json::from_value(json!("intro")).unwrap();
    assert_eq!(kind, types::MarkerKind::Intro);
}

#[test]
fn a_problem_code_is_the_last_segment_of_its_type() {
    let problem: Problem = serde_json::from_value(read("authentication_required.json")).unwrap();
    assert_eq!(problem.code(), "authentication_required");
    assert_eq!(problem.status, 401);
}

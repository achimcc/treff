//! The SCIM door: the identity provider pushes everybody it knows, so the
//! forum knows them before they ever open it (plan-stage-6, ADR 0007).
//!
//! **The bodies here are the ones Authentik's client really builds**, read
//! from its source at 2026.5.6 (`authentik/providers/scim/clients/`), not
//! invented: `POST /Users` with `externalId`, `POST /Groups` with `members`
//! (because treff answers `patch: false`), and then — always, whatever the
//! configuration says — the members again as `PATCH`. A door tested against
//! a guessed client is a door tested against nobody.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

mod common;
use common::{body_of, setup_with_db, signed_in};
use treff::web::scim::{CONTENT_TYPE, GROUPS, SERVICE_PROVIDER_CONFIG, USERS};

const SCIM: &str = "scim-token-0123456789";
const BELL: &str = "bell-token-9876543210";
const FORUM: &str = "forum.example.org";

// The provider's ids are UUIDs, and treff's mapping makes the user's UUID the
// `externalId` — which is the OIDC `sub` a later sign-in arrives with.
const KONRAD: &str = "5b1e0c1c-1111-4a4a-9b9b-000000000001";
const ADA: &str = "5b1e0c1c-1111-4a4a-9b9b-000000000002";
const HOUSEHOLD: &str = "5b1e0c1c-2222-4a4a-9b9b-00000000000a";
const NEIGHBOURS: &str = "5b1e0c1c-2222-4a4a-9b9b-00000000000b";

fn internal(db: &treff::db::Db, scim: Option<&str>) -> axum::Router {
    treff::web::internal::router(treff::web::internal::InternalState::new(
        std::sync::Arc::new(
            treff::config::Config::parse(common::CONFIGURATION).expect("configuration"),
        ),
        db.clone(),
        treff::web::internal::Tokens {
            events: None,
            bell: Some(BELL.into()),
            scim: scim.map(|t| t.as_bytes().to_vec()),
        },
    ))
}

fn at(method: &str, uri: &str, token: &str, body: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("accept", CONTENT_TYPE);
    if body.is_some() {
        builder = builder.header("content-type", CONTENT_TYPE);
    }
    builder
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .expect("request")
}

async fn send(app: &axum::Router, request: Request<Body>) -> axum::response::Response {
    app.clone().oneshot(request).await.expect("response")
}

async fn json(response: axum::response::Response) -> serde_json::Value {
    let text = body_of(response).await;
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("not json ({e}): {text}"))
}

/// A user as `SCIMUserClient.create` dumps it: `exclude_unset`, so only what
/// the mapping produced, plus the `externalId` treff's own mapping sets.
fn user(user_name: &str, display: &str, email: &str, active: bool, external: &str) -> String {
    format!(
        r#"{{"schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
             "userName": "{user_name}", "displayName": "{display}",
             "name": {{"formatted": "{display}"}},
             "emails": [{{"value": "{email}", "type": "other", "primary": true}}],
             "active": {active}, "externalId": "{external}"}}"#
    )
}

/// The same, as `update` sends it: with `id` set to what treff answered.
fn user_put(id: &str, user_name: &str, display: &str, email: &str, active: bool) -> String {
    let body = user(user_name, display, email, active, id);
    format!(r#"{{"id": "{id}", {}"#, &body[1..])
}

/// A group as `SCIMGroupClient.create` dumps it. `members` is in there
/// BECAUSE treff answers `patch: false` — with `patch: true` the client
/// deletes the field and sends the members only as `PATCH`.
fn group(display: &str, external: &str, members: &[&str]) -> String {
    let members: Vec<String> = members
        .iter()
        .map(|m| format!(r#"{{"value": "{m}"}}"#))
        .collect();
    format!(
        r#"{{"schemas": ["urn:ietf:params:scim:schemas:core:2.0:Group"],
             "displayName": "{display}", "externalId": "{external}",
             "members": [{}]}}"#,
        members.join(", ")
    )
}

/// The same without `members` at all — which is what the client sends for a
/// group whose members it has not synced yet, and what `_update_put` sends
/// for a group that has none. **Absent is not empty.**
fn group_without_members(display: &str, external: &str) -> String {
    format!(
        r#"{{"schemas": ["urn:ietf:params:scim:schemas:core:2.0:Group"],
             "displayName": "{display}", "externalId": "{external}"}}"#
    )
}

/// `_patch_add_users` / `patch_compare_users`: one operation per member, all
/// of them in ONE request — because treff answers `bulk.maxOperations: 0`,
/// and `_patch_chunked` then makes a single chunk of everything.
fn patch_add(members: &[&str]) -> String {
    let ops: Vec<String> = members
        .iter()
        .map(|m| format!(r#"{{"op": "add", "path": "members", "value": [{{"value": "{m}"}}]}}"#))
        .collect();
    format!(
        r#"{{"schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
             "Operations": [{}]}}"#,
        ops.join(", ")
    )
}

/// `patch_compare_users` removes with a value...
fn patch_remove(members: &[&str]) -> String {
    let ops: Vec<String> = members
        .iter()
        .map(|m| format!(r#"{{"op": "remove", "path": "members", "value": [{{"value": "{m}"}}]}}"#))
        .collect();
    format!(
        r#"{{"schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
             "Operations": [{}]}}"#,
        ops.join(", ")
    )
}

/// ...and `_patch_remove_users` with a filter in the path and no value at all
/// (`exclude_none=True` drops it). Same meaning, different shape — a door
/// that knows only one of the two loses members silently.
fn patch_remove_by_filter(members: &[&str]) -> String {
    let ops: Vec<String> = members
        .iter()
        .map(|m| format!(r#"{{"op": "remove", "path": "members[value eq \"{m}\"]"}}"#))
        .collect();
    format!(
        r#"{{"schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
             "Operations": [{}]}}"#,
        ops.join(", ")
    )
}

async fn row(
    db: &treff::db::Db,
    subject: &str,
) -> Option<(Option<String>, Option<String>, String)> {
    use sqlx::Row;
    let row = sqlx::query("SELECT handle, email, groups_json FROM accounts WHERE subject = ?")
        .bind(subject)
        .fetch_optional(db.pool())
        .await
        .expect("query")?;
    Some((row.get("handle"), row.get("email"), row.get("groups_json")))
}

async fn accounts(db: &treff::db::Db) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM accounts")
        .fetch_one(db.pool())
        .await
        .expect("count")
}

/// EVERY REQUIRED FIELD, BECAUSE A MISSING ONE IS SILENT. Authentik validates
/// this answer against its `ServiceProviderConfiguration`; if validation
/// fails it logs a warning and carries on with its own defaults — so a
/// forgotten `bulk.maxOperations` does not break anything visibly, it just
/// makes treff's answer count for nothing.
#[tokio::test]
async fn the_provider_is_told_what_this_door_can_do() {
    let (_dir, db, _public) = setup_with_db().await;
    let app = internal(&db, Some(SCIM));
    let answer = send(&app, at("GET", SERVICE_PROVIDER_CONFIG, SCIM, None)).await;
    assert_eq!(answer.status(), StatusCode::OK);
    assert_eq!(answer.headers()["content-type"], CONTENT_TYPE);
    let config = json(answer).await;

    assert_eq!(config["patch"]["supported"], false, "{config}");
    assert_eq!(config["bulk"]["supported"], false, "{config}");
    assert_eq!(
        config["bulk"]["maxOperations"], 0,
        "everything in one request"
    );
    assert_eq!(
        config["filter"]["supported"], true,
        "after a 409 it searches"
    );
    assert_eq!(config["sort"]["supported"], false, "{config}");
    assert_eq!(config["changePassword"]["supported"], false, "{config}");
    let schemes = config["authenticationSchemes"]
        .as_array()
        .expect("authenticationSchemes is a list");
    assert!(!schemes.is_empty(), "{config}");
    for scheme in schemes {
        assert!(scheme["name"].is_string(), "{scheme}");
        assert!(scheme["description"].is_string(), "{scheme}");
    }
}

/// THE WHOLE SEQUENCE, IN THE ORDER AUTHENTIK RUNS IT — and at the end of it
/// somebody who never opened the forum is in its `@` list.
#[tokio::test]
async fn the_sequence_the_provider_runs_ends_with_a_person_the_forum_knows() {
    let (dir, db, public) = setup_with_db().await;
    let app = internal(&db, Some(SCIM));

    // 1. What can this door do?
    assert_eq!(
        send(&app, at("GET", SERVICE_PROVIDER_CONFIG, SCIM, None))
            .await
            .status(),
        StatusCode::OK
    );

    // 2. The people.
    let created = send(
        &app,
        at(
            "POST",
            USERS,
            SCIM,
            Some(&user("Ada", "Ada Lovelace", "ada@example.org", true, ADA)),
        ),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let body = json(created).await;
    // The client stores THIS id and addresses everything else by it. An empty
    // one stops its sync with "missing or invalid `id`".
    assert_eq!(body["id"], ADA, "{body}");
    assert_eq!(body["externalId"], ADA, "{body}");
    assert_eq!(body["userName"], "Ada", "{body}");
    assert_eq!(body["active"], true, "{body}");

    send(
        &app,
        at(
            "POST",
            USERS,
            SCIM,
            Some(&user(
                "Konrad",
                "Konrad Zuse",
                "konrad@example.org",
                true,
                KONRAD,
            )),
        ),
    )
    .await;

    // 3. The group, with the members it already knows...
    let created = send(
        &app,
        at(
            "POST",
            GROUPS,
            SCIM,
            Some(&group("Household", HOUSEHOLD, &[ADA])),
        ),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let body = json(created).await;
    assert_eq!(body["id"], HOUSEHOLD, "{body}");
    assert_eq!(body["displayName"], "Household", "{body}");

    // 4. ...and then the members AGAIN as a PATCH, which `create` always
    //    sends. Adding somebody twice must not be an error.
    let patched = send(
        &app,
        at(
            "PATCH",
            &format!("{GROUPS}/{HOUSEHOLD}"),
            SCIM,
            Some(&patch_add(&[ADA])),
        ),
    )
    .await;
    assert!(patched.status().is_success(), "{}", patched.status());

    // 5. `patch_compare_users` reads the group back and compares.
    let read = send(
        &app,
        at("GET", &format!("{GROUPS}/{HOUSEHOLD}"), SCIM, None),
    )
    .await;
    assert_eq!(read.status(), StatusCode::OK);
    let body = json(read).await;
    let members: Vec<&str> = body["members"]
        .as_array()
        .expect("members")
        .iter()
        .filter_map(|m| m["value"].as_str())
        .collect();
    assert_eq!(members, vec![ADA], "{body}");

    // 6. Konrad joins a group that may read nothing of this forum.
    send(
        &app,
        at(
            "POST",
            GROUPS,
            SCIM,
            Some(&group("Neighbours", NEIGHBOURS, &[KONRAD])),
        ),
    )
    .await;

    assert_eq!(row(&db, ADA).await.expect("ada").2, r#"["Household"]"#);
    assert_eq!(
        row(&db, KONRAD).await.expect("konrad").2,
        r#"["Neighbours"]"#
    );

    // AND THE POINT OF THE WHOLE STAGE: neither of them has ever been here,
    // and the forum offers the one who may read it.
    let viewer = signed_in(&db, dir.path(), "viewer", &["Household"]).await;
    let list = json(
        public
            .oneshot(
                Request::builder()
                    .uri("/mentionable")
                    .header("host", FORUM)
                    .header("cookie", &viewer)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response"),
    )
    .await;
    let handles: Vec<&str> = list
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(|o| o["handle"].as_str())
        .collect();
    assert!(handles.contains(&"ada"), "{list}");
    assert!(!handles.contains(&"konrad"), "the neighbour: {list}");
    assert!(!handles.contains(&"viewer"), "whoever asks: {list}");
}

/// A RESYNC IS NOT A CONFLICT. `POST` on somebody who is already there writes
/// and answers like a creation — so the client never needs its 409 detour,
/// and two syncs leave one row.
#[tokio::test]
async fn a_second_post_writes_the_same_row() {
    let (_dir, db, _public) = setup_with_db().await;
    let app = internal(&db, Some(SCIM));
    let first = user("Ada", "Ada Lovelace", "ada@example.org", true, ADA);
    let again = user("Ada", "Ada, Countess", "ada@example.org", true, ADA);
    for body in [&first, &again] {
        let r = send(&app, at("POST", USERS, SCIM, Some(body))).await;
        assert_eq!(r.status(), StatusCode::CREATED, "{body}");
    }
    assert_eq!(accounts(&db).await, 1);

    for body in [
        group("Household", HOUSEHOLD, &[ADA]),
        group("Household", HOUSEHOLD, &[ADA]),
    ] {
        let r = send(&app, at("POST", GROUPS, SCIM, Some(&body))).await;
        assert_eq!(r.status(), StatusCode::CREATED);
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM scim_groups")
        .fetch_one(db.pool())
        .await
        .expect("count");
    assert_eq!(n, 1);
}

/// THE THREE MEMBER FORMS ARE ALL THREE, AND THEY ARE ALL THERE IS. Anything
/// else — the whole-group `replace` the client sends to providers that
/// announce `patch: true`, an unknown path, an unknown operation — is
/// refused, and refused without changing anything.
#[tokio::test]
async fn the_three_member_forms_and_nothing_else() {
    let (_dir, db, _public) = setup_with_db().await;
    let app = internal(&db, Some(SCIM));
    let patch = |body: String| {
        let app = app.clone();
        async move {
            send(
                &app,
                at("PATCH", &format!("{GROUPS}/{HOUSEHOLD}"), SCIM, Some(&body)),
            )
            .await
        }
    };
    send(
        &app,
        at(
            "POST",
            USERS,
            SCIM,
            Some(&user("Ada", "Ada", "a@example.org", true, ADA)),
        ),
    )
    .await;
    send(
        &app,
        at(
            "POST",
            USERS,
            SCIM,
            Some(&user("Konrad", "Konrad", "k@example.org", true, KONRAD)),
        ),
    )
    .await;
    send(
        &app,
        at(
            "POST",
            GROUPS,
            SCIM,
            Some(&group("Household", HOUSEHOLD, &[])),
        ),
    )
    .await;

    assert!(patch(patch_add(&[ADA, KONRAD])).await.status().is_success());
    assert_eq!(row(&db, ADA).await.expect("ada").2, r#"["Household"]"#);
    assert_eq!(row(&db, KONRAD).await.expect("k").2, r#"["Household"]"#);

    assert!(patch(patch_remove(&[KONRAD])).await.status().is_success());
    assert_eq!(row(&db, KONRAD).await.expect("k").2, "[]");

    assert!(
        patch(patch_remove_by_filter(&[ADA]))
            .await
            .status()
            .is_success()
    );
    assert_eq!(row(&db, ADA).await.expect("ada").2, "[]");

    // Back in, so that the refusals below have something to destroy.
    assert!(patch(patch_add(&[ADA])).await.status().is_success());

    for body in [
        // The whole-group replace (`_update_patch_general`).
        r#"{"Operations": [{"op": "replace", "value": {"displayName": "Taken over"}}]}"#
            .to_string(),
        r#"{"Operations": [{"op": "replace", "path": "displayName", "value": "Taken over"}]}"#
            .to_string(),
        r#"{"Operations": [{"op": "add", "path": "displayName", "value": "x"}]}"#.to_string(),
        r#"{"Operations": [{"op": "delete", "path": "members", "value": [{"value": "x"}]}]}"#
            .to_string(),
        r#"{"Operations": [{"op": "remove", "path": "members[displayName eq \"x\"]"}]}"#
            .to_string(),
        // ONE BAD ONE SPOILS THE WHOLE REQUEST: a chunk is applied or it is not.
        format!(
            r#"{{"Operations": [{{"op": "remove", "path": "members", "value": [{{"value": "{ADA}"}}]}},
                                {{"op": "replace", "path": "displayName", "value": "x"}}]}}"#
        ),
        r#"not json"#.to_string(),
        r#"{"Operations": []}"#.to_string(),
    ] {
        let r = patch(body.clone()).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST, "{body}");
    }
    assert_eq!(
        row(&db, ADA).await.expect("ada").2,
        r#"["Household"]"#,
        "nothing a refusal did survived"
    );
    let name: String = sqlx::query_scalar("SELECT name FROM scim_groups WHERE id = ?")
        .bind(HOUSEHOLD)
        .fetch_one(db.pool())
        .await
        .expect("name");
    assert_eq!(name, "Household");
}

/// `PUT /Groups/{id}` IS THE MAIN PATH HERE, precisely because treff answers
/// `patch: false`: the client's `update` then goes through `_update_put`,
/// which sends the whole group — and afterwards compares the members again.
#[tokio::test]
async fn a_put_replaces_the_members_and_a_put_without_them_leaves_them() {
    let (_dir, db, _public) = setup_with_db().await;
    let app = internal(&db, Some(SCIM));
    let put = |body: String| {
        let app = app.clone();
        async move {
            send(
                &app,
                at("PUT", &format!("{GROUPS}/{HOUSEHOLD}"), SCIM, Some(&body)),
            )
            .await
        }
    };
    for (name, display, id) in [("Ada", "Ada", ADA), ("Konrad", "Konrad", KONRAD)] {
        send(
            &app,
            at(
                "POST",
                USERS,
                SCIM,
                Some(&user(name, display, "a@example.org", true, id)),
            ),
        )
        .await;
    }
    send(
        &app,
        at(
            "POST",
            GROUPS,
            SCIM,
            Some(&group("Household", HOUSEHOLD, &[ADA])),
        ),
    )
    .await;

    let answer = put(group("Household", HOUSEHOLD, &[KONRAD])).await;
    assert_eq!(answer.status(), StatusCode::OK);
    let body = json(answer).await;
    let members: Vec<&str> = body["members"]
        .as_array()
        .expect("members")
        .iter()
        .filter_map(|m| m["value"].as_str())
        .collect();
    assert_eq!(members, vec![KONRAD], "{body}");
    assert_eq!(row(&db, ADA).await.expect("ada").2, "[]", "replaced");

    // ABSENT IS NOT EMPTY. The client leaves `members` out for a group whose
    // people it has not synced yet, and removes the rest by PATCH afterwards
    // — a door that read that as "no members" would empty the group between
    // two requests, and `may_read` with it.
    assert_eq!(
        put(group_without_members("Household", HOUSEHOLD))
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(row(&db, KONRAD).await.expect("k").2, r#"["Household"]"#);

    // A rename moves everybody in it, because the NAME is what treff stores.
    assert_eq!(
        put(group_without_members("Haushalt", HOUSEHOLD))
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(row(&db, KONRAD).await.expect("k").2, r#"["Haushalt"]"#);

    // And a rename onto a name another group holds is a conflict, not a
    // silent merge of two groups into one.
    send(
        &app,
        at(
            "POST",
            GROUPS,
            SCIM,
            Some(&group_without_members("Neighbours", NEIGHBOURS)),
        ),
    )
    .await;
    assert_eq!(
        put(group_without_members("Neighbours", HOUSEHOLD))
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(row(&db, KONRAD).await.expect("k").2, r#"["Haushalt"]"#);
}

/// AN ID THAT IS NOT A UUID IS NOT AN ID HERE. treff's `subject` is the OIDC
/// `sub`, and a door that took any string would quietly build a second,
/// parallel population that no sign-in ever matches.
#[tokio::test]
async fn an_id_that_is_not_a_uuid_leaves_no_row() {
    let (_dir, db, _public) = setup_with_db().await;
    let app = internal(&db, Some(SCIM));
    for external in ["", "ada", "5b1e0c1c-1111-4a4a-9b9b", "../../etc/passwd"] {
        let body = user("Ada", "Ada", "ada@example.org", true, external);
        let r = send(&app, at("POST", USERS, SCIM, Some(&body))).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST, "{external:?}");
    }
    // A user name that is no handle is NOT an error — the person exists, they
    // are simply not mentionable.
    let body = user("Ada Lovelace", "Ada", "ada@example.org", true, ADA);
    assert_eq!(
        send(&app, at("POST", USERS, SCIM, Some(&body)))
            .await
            .status(),
        StatusCode::CREATED
    );
    assert_eq!(row(&db, ADA).await.expect("ada").0, None, "no handle");

    // And a member that is not a UUID is refused, whole request, no change.
    send(
        &app,
        at(
            "POST",
            GROUPS,
            SCIM,
            Some(&group("Household", HOUSEHOLD, &[])),
        ),
    )
    .await;
    let r = send(
        &app,
        at(
            "PATCH",
            &format!("{GROUPS}/{HOUSEHOLD}"),
            SCIM,
            Some(&patch_add(&["not-a-uuid"])),
        ),
    )
    .await;
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM scim_members")
        .fetch_one(db.pool())
        .await
        .expect("count");
    assert_eq!(n, 0);

    // A group whose own id is no UUID, likewise.
    let r = send(
        &app,
        at("POST", GROUPS, SCIM, Some(&group("Others", "g-1", &[]))),
    )
    .await;
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    assert_eq!(accounts(&db).await, 1, "only Ada");
}

/// LEAVING CLEARS, IT DOES NOT ERASE. A deactivated or deleted person keeps
/// their row — the posts carry `author_subject` — and loses handle, address
/// and every group, so they are neither mentionable nor mailed.
#[tokio::test]
async fn leaving_clears_the_row_and_keeps_it() {
    let (_dir, db, _public) = setup_with_db().await;
    let app = internal(&db, Some(SCIM));
    send(
        &app,
        at(
            "POST",
            USERS,
            SCIM,
            Some(&user("Ada", "Ada", "ada@example.org", true, ADA)),
        ),
    )
    .await;
    send(
        &app,
        at(
            "POST",
            GROUPS,
            SCIM,
            Some(&group("Household", HOUSEHOLD, &[ADA])),
        ),
    )
    .await;
    assert_eq!(
        row(&db, ADA).await.expect("ada"),
        (
            Some("ada".into()),
            Some("ada@example.org".into()),
            r#"["Household"]"#.into()
        )
    );

    // `update` PUTs by the id treff answered with.
    let off = user_put(ADA, "Ada", "Ada", "ada@example.org", false);
    let answer = send(&app, at("PUT", &format!("{USERS}/{ADA}"), SCIM, Some(&off))).await;
    assert_eq!(answer.status(), StatusCode::OK);
    assert_eq!(json(answer).await["active"], false);
    assert_eq!(
        row(&db, ADA).await.expect("still there"),
        (None, None, "[]".into())
    );

    let on = user_put(ADA, "Ada", "Ada", "ada@example.org", true);
    send(&app, at("PUT", &format!("{USERS}/{ADA}"), SCIM, Some(&on))).await;
    assert_eq!(
        row(&db, ADA).await.expect("back").2,
        r#"["Household"]"#,
        "back in the group they never left"
    );

    let gone = send(&app, at("DELETE", &format!("{USERS}/{ADA}"), SCIM, None)).await;
    assert_eq!(gone.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        row(&db, ADA).await.expect("kept"),
        (None, None, "[]".into())
    );
    assert_eq!(accounts(&db).await, 1);
    assert_eq!(
        send(&app, at("DELETE", &format!("{USERS}/{ADA}"), SCIM, None))
            .await
            .status(),
        StatusCode::NOT_FOUND,
        "gone is gone"
    );

    let gone = send(
        &app,
        at("DELETE", &format!("{GROUPS}/{HOUSEHOLD}"), SCIM, None),
    )
    .await;
    assert_eq!(gone.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        send(
            &app,
            at("GET", &format!("{GROUPS}/{HOUSEHOLD}"), SCIM, None)
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

/// `discover` walks the whole population by `startIndex` and stops when it has
/// seen `totalResults`; the filter is what it falls back to after a 409.
#[tokio::test]
async fn discovery_pages_and_the_filter_finds_one() {
    let (_dir, db, _public) = setup_with_db().await;
    let app = internal(&db, Some(SCIM));
    send(
        &app,
        at(
            "POST",
            USERS,
            SCIM,
            Some(&user("Ada", "Ada", "ada@example.org", true, ADA)),
        ),
    )
    .await;
    send(
        &app,
        at(
            "POST",
            USERS,
            SCIM,
            Some(&user("Konrad", "Konrad", "k@example.org", true, KONRAD)),
        ),
    )
    .await;
    send(
        &app,
        at(
            "POST",
            GROUPS,
            SCIM,
            Some(&group("Household", HOUSEHOLD, &[ADA])),
        ),
    )
    .await;

    let all = json(send(&app, at("GET", USERS, SCIM, None)).await).await;
    assert_eq!(all["totalResults"], 2, "{all}");
    assert_eq!(all["startIndex"], 1, "{all}");
    assert_eq!(all["Resources"].as_array().expect("Resources").len(), 2);
    assert_eq!(all["itemsPerPage"], 2, "{all}");

    let page = json(
        send(
            &app,
            at("GET", &format!("{USERS}?startIndex=2&count=1"), SCIM, None),
        )
        .await,
    )
    .await;
    assert_eq!(page["totalResults"], 2, "{page}");
    assert_eq!(page["startIndex"], 2, "{page}");
    assert_eq!(page["Resources"].as_array().expect("Resources").len(), 1);

    let found = json(
        send(
            &app,
            at(
                "GET",
                &format!("{USERS}?filter=userName%20eq%20%22Konrad%22"),
                SCIM,
                None,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(found["totalResults"], 1, "{found}");
    assert_eq!(found["Resources"][0]["id"], KONRAD, "{found}");

    let found = json(
        send(
            &app,
            at(
                "GET",
                &format!("{GROUPS}?filter=displayName%20eq%20%22Household%22"),
                SCIM,
                None,
            ),
        )
        .await,
    )
    .await;
    assert_eq!(found["totalResults"], 1, "{found}");
    assert_eq!(found["Resources"][0]["id"], HOUSEHOLD, "{found}");

    // A filter treff does not understand is said so, not silently ignored —
    // an ignored filter answers "everybody", and the client takes the first.
    for query in [
        "filter=userName%20co%20%22Kon%22",
        "filter=displayName%20eq%20%22Household%22",
        "filter=nonsense",
    ] {
        let r = send(&app, at("GET", &format!("{USERS}?{query}"), SCIM, None)).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST, "{query}");
    }

    let one = json(send(&app, at("GET", &format!("{USERS}/{ADA}"), SCIM, None)).await).await;
    assert_eq!(one["id"], ADA, "{one}");
    assert_eq!(
        send(&app, at("GET", &format!("{USERS}/{KONRAD}xyz"), SCIM, None))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(
            &app,
            at(
                "GET",
                &format!("{USERS}/5b1e0c1c-9999-4a4a-9b9b-000000000009"),
                SCIM,
                None
            )
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

/// ITS OWN TOKEN, AND NO OTHER DOOR'S. A leak of the bell's token must not
/// let anybody write the population of the forum.
#[tokio::test]
async fn the_door_opens_to_its_own_token_only() {
    let (_dir, db, _public) = setup_with_db().await;
    let app = internal(&db, Some(SCIM));
    let body = user("Ada", "Ada", "ada@example.org", true, ADA);
    for token in [
        "wrong",
        BELL,
        "",
        "scim-token-012345678",
        "scim-token-01234567899",
    ] {
        let r = send(&app, at("POST", USERS, token, Some(&body))).await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{token:?}");
        let r = send(&app, at("GET", SERVICE_PROVIDER_CONFIG, token, None)).await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{token:?}");
    }
    let bare = Request::builder()
        .method("POST")
        .uri(USERS)
        .header("content-type", CONTENT_TYPE)
        .body(Body::from(body))
        .expect("request");
    assert_eq!(send(&app, bare).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(accounts(&db).await, 0);
}

/// A DOOR WHOSE TOKEN WAS NEVER SET DOES NOT EXIST — never "open, because
/// nothing was configured".
#[tokio::test]
async fn without_a_token_the_door_is_not_there() {
    let (_dir, db, _public) = setup_with_db().await;
    let app = internal(&db, None);
    for (method, uri) in [
        ("GET", SERVICE_PROVIDER_CONFIG),
        ("POST", USERS),
        ("GET", USERS),
        ("POST", GROUPS),
    ] {
        let r = send(&app, at(method, uri, "", Some("{}"))).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "{method} {uri}");
    }
    // And the bell, whose token IS set, still answers — one shut door does
    // not shut the others.
    let bell = Request::builder()
        .uri("/internal/bell")
        .header("authorization", format!("Bearer {BELL}"))
        .header("x-treff-user", "ada")
        .header("x-treff-groups", "Household")
        .body(Body::empty())
        .expect("request");
    assert_eq!(send(&app, bell).await.status(), StatusCode::OK);
}

/// The population of the forum is not writable from the internet. Whatever
/// the public router answers under these paths, it is not this.
#[tokio::test]
async fn the_public_router_has_no_scim_routes() {
    let (_dir, db, public) = setup_with_db().await;
    for (method, uri) in [("POST", USERS), ("GET", SERVICE_PROVIDER_CONFIG)] {
        let r = public
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("host", FORUM)
                    .header("authorization", format!("Bearer {SCIM}"))
                    .header("content-type", CONTENT_TYPE)
                    .body(Body::from(user("Ada", "Ada", "a@example.org", true, ADA)))
                    .expect("request"),
            )
            .await
            .expect("response");
        // Whatever it is — a refusal, or the redirect to the sign-in that
        // every unknown path there gets — it is not this door doing anything.
        assert!(!r.status().is_success(), "{method} {uri}: {}", r.status());
    }
    assert_eq!(accounts(&db).await, 0);
}

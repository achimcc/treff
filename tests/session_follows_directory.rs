//! Homeserver audit 3, B93: leaving a group, being deactivated or deleted
//! over SCIM ends what an existing session may do — at once, not after the
//! twelve hours the cookie would otherwise live.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;
mod common;
use common::{setup_with_db, signed_in};

const SCIM: &str = "scim-token-0123456789";
const KONRAD: &str = "5b1e0c1c-1111-4a4a-9b9b-000000000001";
const HOUSEHOLD: &str = "5b1e0c1c-2222-4a4a-9b9b-00000000000a";

fn internal(db: &treff::db::Db) -> axum::Router {
    treff::web::internal::router(treff::web::internal::InternalState::new(
        std::sync::Arc::new(treff::config::Config::parse(common::CONFIGURATION).unwrap()),
        db.clone(),
        treff::web::internal::Tokens {
            events: None,
            bell: None,
            scim: Some(SCIM.as_bytes().to_vec()),
        },
    ))
}

async fn scim(app: &axum::Router, method: &str, uri: &str, body: Option<String>) -> StatusCode {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {SCIM}"))
        .header("content-type", "application/scim+json");
    let _ = &mut b;
    app.clone()
        .oneshot(b.body(body.map_or_else(Body::empty, Body::from)).unwrap())
        .await
        .unwrap()
        .status()
}

async fn forum(app: &axum::Router, cookie: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .uri("/")
                .header("host", "forum.example.org")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn scim_changes_reach_a_running_session() {
    let (dir, db, app) = setup_with_db().await;
    let intern = internal(&db);
    let user = |active: bool| {
        format!(
            r#"{{"schemas":["urn:ietf:params:scim:schemas:core:2.0:User"],"userName":"konrad","displayName":"Konrad","active":{active},"externalId":"{KONRAD}"}}"#
        )
    };
    assert_eq!(
        scim(&intern, "POST", "/scim/v2/Users", Some(user(true))).await,
        StatusCode::CREATED
    );
    let group = format!(
        r#"{{"displayName":"Household","externalId":"{HOUSEHOLD}","members":[{{"value":"{KONRAD}"}}]}}"#
    );
    assert_eq!(
        scim(&intern, "POST", "/scim/v2/Groups", Some(group)).await,
        StatusCode::CREATED
    );

    let cookie = signed_in(&db, dir.path(), KONRAD, &["Household"]).await;
    assert_eq!(
        forum(&app, &cookie).await,
        StatusCode::OK,
        "before: signed in and a member"
    );

    // 1) removed from the group
    let rm =
        format!(r#"{{"Operations":[{{"op":"remove","path":"members[value eq \"{KONRAD}\"]"}}]}}"#);
    assert_eq!(
        scim(
            &intern,
            "PATCH",
            &format!("/scim/v2/Groups/{HOUSEHOLD}"),
            Some(rm)
        )
        .await,
        StatusCode::OK
    );
    assert_ne!(
        forum(&app, &cookie).await,
        StatusCode::OK,
        "after leaving the group"
    );
    // 2) deactivated
    assert_eq!(
        scim(
            &intern,
            "PUT",
            &format!("/scim/v2/Users/{KONRAD}"),
            Some(user(false))
        )
        .await,
        StatusCode::OK
    );
    assert_ne!(
        forum(&app, &cookie).await,
        StatusCode::OK,
        "after active=false"
    );
    // 3) deleted
    assert_eq!(
        scim(&intern, "DELETE", &format!("/scim/v2/Users/{KONRAD}"), None).await,
        StatusCode::NO_CONTENT
    );
    assert_ne!(forum(&app, &cookie).await, StatusCode::OK, "after DELETE");
}

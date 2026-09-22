//! `/scim/v2`: the door the identity provider pushes its population through
//! (plan-stage-6, ADR 0007).
//!
//! treff used to learn a person's handle and groups at a SIGN-IN, and only
//! then. A mention is for somebody who is not looking, so the forum has to
//! know people before they come — and the provider already knows them all.
//!
//! **It lives on the internal listener (ADR 0006), behind its OWN token.** A
//! leak of the bell's token must not let anybody rewrite who exists; a door
//! whose token was never configured is not mounted at all.
//!
//! **What it accepts is what Authentik's client really sends**, read from its
//! source (2026.5.6, `authentik/providers/scim/clients/`) rather than assumed:
//!
//! * `GET /ServiceProviderConfig` decides how the client behaves afterwards.
//!   treff answers `patch: false` — so groups arrive complete, by `PUT` — and
//!   `bulk.maxOperations: 0`, which makes the client put every operation of a
//!   change into ONE request. **A field missing from that answer is not an
//!   error anywhere**: the client logs a warning and falls back to its own
//!   defaults, which is the same shape with `filter` off. Hence the test that
//!   names every field it validates.
//! * Members nevertheless come as `PATCH`, always: `create` calls
//!   `_patch_add_users` whatever the configuration said, and `update` calls
//!   `patch_compare_users`, which reads the group back with `GET /Groups/{id}`
//!   and sends the differences. Three shapes exist and no more — `add` on
//!   `members`, `remove` on `members`, and `remove` on
//!   `members[value eq "…"]` with no value at all. Anything else is refused.
//! * `POST` on somebody who is already here writes and answers like a
//!   creation. The alternative is `409`, after which the client searches by
//!   filter and takes the first hit — a detour with two more ways to go wrong,
//!   for a case that is simply "sync ran twice".
//!
//! **The id is the `externalId`, and it must be a UUID.** treff's OIDC
//! provider hands out `sub = user.uuid`, and the operator's SCIM mapping sets
//! `externalId` to the same value; that is what makes the person the SCIM door
//! writes and the person who later signs in ONE row. Authentik's own default
//! would be `str(user.uid)`, a hash — so a non-UUID id is not a malformed
//! request to work around, it is the mapping being wrong, and it is refused
//! loudly rather than building a second population nobody ever matches.

use crate::db::directory::{self, Group, User};
use crate::web::internal::InternalState;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};

/// Every route is under this, so the operator configures one URL.
macro_rules! under {
    ($tail:literal) => {
        concat!("/scim/v2", $tail)
    };
}

pub const SERVICE_PROVIDER_CONFIG: &str = under!("/ServiceProviderConfig");
pub const USERS: &str = under!("/Users");
pub const GROUPS: &str = under!("/Groups");
const ONE_USER: &str = under!("/Users/{id}");
const ONE_GROUP: &str = under!("/Groups/{id}");

pub const CONTENT_TYPE: &str = "application/scim+json";

const USER_SCHEMA: &str = "urn:ietf:params:scim:schemas:core:2.0:User";
const GROUP_SCHEMA: &str = "urn:ietf:params:scim:schemas:core:2.0:Group";
const LIST_SCHEMA: &str = "urn:ietf:params:scim:api:messages:2.0:ListResponse";
const ERROR_SCHEMA: &str = "urn:ietf:params:scim:api:messages:2.0:Error";

/// How many resources one page holds when the client does not say. Its
/// `discover` never says, and walks by `startIndex` until it has seen
/// `totalResults`.
const PER_PAGE: i64 = 100;
const MOST_PER_PAGE: i64 = 200;

pub fn routes(state: &InternalState) -> Router<InternalState> {
    Router::new()
        .route(SERVICE_PROVIDER_CONFIG, get(service_provider_config))
        .route(USERS, post(create_user).get(list_users))
        .route(ONE_USER, get(read_user).put(write_user).delete(remove_user))
        .route(GROUPS, post(create_group).get(list_groups))
        .route(
            ONE_GROUP,
            get(read_group)
                .put(write_group)
                .patch(patch_group)
                .delete(remove_group),
        )
        // One guard for all of them, rather than a line every handler could
        // be written without.
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            demand_the_token,
        ))
}

async fn demand_the_token(
    State(state): State<InternalState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let Some(token) = state.tokens.scim.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !crate::web::internal::presents(request.headers(), token) {
        return refusal(
            StatusCode::UNAUTHORIZED,
            None,
            "a bearer token for /scim/v2 is required",
        );
    }
    next.run(request).await
}

// --- Answers ----------------------------------------------------------------

fn answer(status: StatusCode, body: &Value) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, CONTENT_TYPE)
        .header(header::CACHE_CONTROL, "no-store")
        .body(axum::body::Body::from(body.to_string()))
        .map_or_else(
            |_| StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            IntoResponse::into_response,
        )
}

fn refusal(status: StatusCode, scim_type: Option<&str>, detail: &str) -> Response {
    let mut body = json!({
        "schemas": [ERROR_SCHEMA],
        "status": status.as_u16().to_string(),
        "detail": detail,
    });
    if let Some(kind) = scim_type {
        body["scimType"] = kind.into();
    }
    answer(status, &body)
}

/// Why something was refused: the SCIM `scimType` and a sentence for the
/// operator's log. Nothing here says who exists.
struct Refused {
    scim_type: &'static str,
    detail: String,
}

fn refused(scim_type: &'static str, detail: impl Into<String>) -> Refused {
    Refused {
        scim_type,
        detail: detail.into(),
    }
}

type Checked<T> = Result<T, Refused>;

fn bad_request(why: &Refused) -> Response {
    refusal(StatusCode::BAD_REQUEST, Some(why.scim_type), &why.detail)
}

fn not_found(what: &str) -> Response {
    refusal(StatusCode::NOT_FOUND, None, &format!("no such {what}"))
}

fn broke(what: &str, e: &anyhow::Error) -> Response {
    eprintln!("treff: scim: cannot {what}: {e}");
    refusal(
        StatusCode::INTERNAL_SERVER_ERROR,
        None,
        &format!("cannot {what}"),
    )
}

// --- What an id is ----------------------------------------------------------

/// A UUID and nothing else, lower-cased.
///
/// Lower-casing is not cosmetic: the value has to equal the `sub` of a later
/// sign-in, and two spellings of one UUID would be two people.
fn checked_id(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let bytes = raw.as_bytes();
    let shaped = bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        });
    shaped.then(|| raw.to_ascii_lowercase())
}

fn as_id(raw: Option<&str>, what: &str) -> Checked<String> {
    let given = raw.unwrap_or_default();
    checked_id(given).ok_or_else(|| {
        refused(
            "invalidValue",
            format!(
                "the {what} must be a UUID — treff's `sub` is the user's UUID, and the SCIM \
                 mapping has to set `externalId` to it"
            ),
        )
    })
}

// --- What comes in ----------------------------------------------------------
//
// Unknown fields are ignored on purpose: SCIM says a server may be sent
// extensions, and refusing them would make treff break on a provider's next
// release. What treff TAKES goes through the same checks a sign-in goes
// through.

#[derive(serde::Deserialize)]
struct EmailIn {
    value: Option<String>,
    primary: Option<bool>,
}

#[derive(serde::Deserialize)]
struct UserIn {
    id: Option<String>,
    #[serde(rename = "externalId")]
    external_id: Option<String>,
    #[serde(rename = "userName")]
    user_name: Option<String>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    emails: Option<Vec<EmailIn>>,
    active: Option<bool>,
}

#[derive(serde::Deserialize)]
struct MemberIn {
    value: Option<String>,
}

#[derive(serde::Deserialize)]
struct GroupIn {
    id: Option<String>,
    #[serde(rename = "externalId")]
    external_id: Option<String>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    members: Option<Vec<MemberIn>>,
}

#[derive(serde::Deserialize)]
struct OperationIn {
    op: Option<String>,
    path: Option<String>,
    value: Option<Value>,
}

#[derive(serde::Deserialize)]
struct PatchIn {
    #[serde(rename = "Operations")]
    operations: Option<Vec<OperationIn>>,
}

fn parsed<T: serde::de::DeserializeOwned>(body: &Bytes) -> Checked<T> {
    serde_json::from_slice(body)
        .map_err(|e| refused("invalidSyntax", format!("not a SCIM resource: {e}")))
}

/// The primary address, or the first one that is an address at all.
fn primary_email(emails: Option<Vec<EmailIn>>) -> Option<String> {
    let emails: Vec<String> = emails
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| {
            let value = e.value?;
            let value = value.trim().to_string();
            (!value.is_empty()).then_some((e.primary.unwrap_or(false), value))
        })
        .fold(Vec::new(), |mut acc, (primary, value)| {
            if primary {
                acc.insert(0, value);
            } else {
                acc.push(value);
            }
            acc
        });
    emails.into_iter().next()
}

/// `id` is the path's when there is one — and then the body's own id and
/// `externalId`, where present, have to agree with it. They do agree in every
/// request Authentik builds; a disagreement is drift, and drift written down
/// is a second population.
fn checked_user(raw: UserIn, id: Option<&str>) -> Checked<User> {
    let from_body = raw.external_id.as_deref().or(raw.id.as_deref());
    let id = match id {
        Some(path) => {
            let path = as_id(Some(path), "id in the path")?;
            for (name, given) in [
                ("id", raw.id.as_deref()),
                ("externalId", raw.external_id.as_deref()),
            ] {
                if let Some(given) = given
                    && as_id(Some(given), name)? != path
                {
                    return Err(refused(
                        "invalidValue",
                        format!("the {name} in the body is not the id in the path"),
                    ));
                }
            }
            path
        }
        None => as_id(from_body, "externalId")?,
    };
    let user_name = raw.user_name.unwrap_or_default().trim().to_string();
    if user_name.is_empty() {
        return Err(refused("invalidValue", "a userName is required"));
    }
    let display_name = raw
        .display_name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| user_name.clone());
    Ok(User {
        id,
        user_name,
        display_name,
        email: primary_email(raw.emails),
        active: raw.active.unwrap_or(true),
    })
}

fn checked_members(members: Option<Vec<MemberIn>>) -> Checked<Option<Vec<String>>> {
    let Some(members) = members else {
        // NOT THE SAME AS AN EMPTY LIST. The client leaves `members` out when
        // a group has none *that it has synced yet*, and then removes the
        // rest by PATCH — so "absent" means "do not touch".
        return Ok(None);
    };
    members
        .into_iter()
        .map(|m| as_id(m.value.as_deref(), "member"))
        .collect::<Checked<Vec<String>>>()
        .map(Some)
}

fn checked_group(raw: GroupIn, id: Option<&str>) -> Checked<(String, String, Option<Vec<String>>)> {
    let id = match id {
        Some(path) => as_id(Some(path), "id in the path")?,
        None => as_id(
            raw.external_id.as_deref().or(raw.id.as_deref()),
            "externalId",
        )?,
    };
    let name = raw.display_name.unwrap_or_default().trim().to_string();
    if name.is_empty() {
        return Err(refused("invalidValue", "a displayName is required"));
    }
    Ok((id, name, checked_members(raw.members)?))
}

// --- What goes out ----------------------------------------------------------

fn user_resource(user: &User) -> Value {
    let mut out = json!({
        "schemas": [USER_SCHEMA],
        "id": user.id,
        "externalId": user.id,
        "userName": user.user_name,
        "displayName": user.display_name,
        "active": user.active,
        "meta": {"resourceType": "User", "location": format!("{USERS}/{}", user.id)},
    });
    // An address only when there is one: the client validates every address
    // in an answer it discovers, and an empty string is not one.
    if let Some(email) = &user.email {
        out["emails"] = json!([{"value": email, "type": "other", "primary": true}]);
    }
    out
}

fn group_resource(group: &Group) -> Value {
    json!({
        "schemas": [GROUP_SCHEMA],
        "id": group.id,
        "externalId": group.id,
        "displayName": group.name,
        "members": group.members.iter()
            .map(|m| json!({"value": m, "type": "User"}))
            .collect::<Vec<Value>>(),
        "meta": {"resourceType": "Group", "location": format!("{GROUPS}/{}", group.id)},
    })
}

fn list_resource(total: i64, start: i64, resources: Vec<Value>) -> Value {
    json!({
        "schemas": [LIST_SCHEMA],
        "totalResults": total,
        "startIndex": start,
        "itemsPerPage": resources.len(),
        "Resources": resources,
    })
}

// --- The routes -------------------------------------------------------------

/// What this door can do — and, through the client's own logic, what it will
/// be sent afterwards.
async fn service_provider_config() -> Response {
    answer(
        StatusCode::OK,
        &json!({
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:ServiceProviderConfig"],
            "documentationUri": "https://github.com/achimcc/treff",
            // Groups arrive whole, by PUT. Their MEMBERS arrive by PATCH all
            // the same — that is the client's, not treff's, decision.
            "patch": {"supported": false},
            // 0 means "do not chunk": every operation of one change in one
            // request, which is the only way treff can apply them together.
            "bulk": {"supported": false, "maxOperations": 0, "maxPayloadSize": 0},
            "filter": {"supported": true, "maxResults": MOST_PER_PAGE},
            "changePassword": {"supported": false},
            "sort": {"supported": false},
            "etag": {"supported": false},
            "authenticationSchemes": [{
                "type": "oauthbearertoken",
                "name": "OAuth Bearer Token",
                "description": "Authentication with a bearer token, over the internal listener",
                "specUri": "https://www.rfc-editor.org/info/rfc6750",
                "primary": true,
            }],
            "meta": {"resourceType": "ServiceProviderConfig", "location": SERVICE_PROVIDER_CONFIG},
        }),
    )
}

#[derive(serde::Deserialize)]
struct Page {
    #[serde(rename = "startIndex")]
    start_index: Option<i64>,
    count: Option<i64>,
    filter: Option<String>,
}

impl Page {
    fn start(&self) -> i64 {
        self.start_index.unwrap_or(1).max(1)
    }

    fn count(&self) -> i64 {
        self.count.unwrap_or(PER_PAGE).clamp(0, MOST_PER_PAGE)
    }

    /// The one filter shape the client builds: `<attribute> eq "<value>"`.
    ///
    /// A filter treff does not understand is REFUSED, never ignored. An
    /// ignored filter answers "everybody", and the client takes the first
    /// resource of that answer for the person it was looking for.
    fn equals(&self, attribute: &str) -> Checked<Option<&str>> {
        let Some(filter) = self.filter.as_deref() else {
            return Ok(None);
        };
        filter
            .trim()
            .strip_prefix(attribute)
            .map(str::trim_start)
            .and_then(|rest| rest.strip_prefix("eq"))
            .map(str::trim_start)
            .and_then(|rest| rest.strip_prefix('"'))
            .and_then(|rest| rest.strip_suffix('"'))
            .map(Some)
            .ok_or_else(|| {
                refused(
                    "invalidFilter",
                    format!("the only filter here is `{attribute} eq \"…\"`"),
                )
            })
    }
}

async fn create_user(State(state): State<InternalState>, body: Bytes) -> Response {
    let user = match parsed(&body).and_then(|raw| checked_user(raw, None)) {
        Ok(user) => user,
        Err(why) => return bad_request(&why),
    };
    match directory::put_user(&state.db, &user).await {
        // A SECOND POST IS NOT A CONFLICT, it is the sync running again.
        Ok(()) => answer(StatusCode::CREATED, &user_resource(&user)),
        Err(e) => broke("store a person", &e),
    }
}

async fn write_user(
    State(state): State<InternalState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let user = match parsed(&body).and_then(|raw| checked_user(raw, Some(&id))) {
        Ok(user) => user,
        Err(why) => return bad_request(&why),
    };
    // A PUT on somebody treff does not have writes them, rather than a 404
    // the client would answer by creating the very same row.
    match directory::put_user(&state.db, &user).await {
        Ok(()) => answer(StatusCode::OK, &user_resource(&user)),
        Err(e) => broke("store a person", &e),
    }
}

async fn read_user(State(state): State<InternalState>, Path(id): Path<String>) -> Response {
    let id = match as_id(Some(&id), "id in the path") {
        Ok(id) => id,
        Err(why) => return bad_request(&why),
    };
    match directory::get_user(&state.db, &id).await {
        Ok(Some(user)) => answer(StatusCode::OK, &user_resource(&user)),
        Ok(None) => not_found("user"),
        Err(e) => broke("read a person", &e),
    }
}

async fn remove_user(State(state): State<InternalState>, Path(id): Path<String>) -> Response {
    let id = match as_id(Some(&id), "id in the path") {
        Ok(id) => id,
        Err(why) => return bad_request(&why),
    };
    match directory::delete_user(&state.db, &id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => not_found("user"),
        Err(e) => broke("remove a person", &e),
    }
}

async fn list_users(State(state): State<InternalState>, Query(page): Query<Page>) -> Response {
    let by_name = match page.equals("userName") {
        Ok(f) => f,
        Err(why) => return bad_request(&why),
    };
    match directory::list_users(&state.db, by_name, page.start(), page.count()).await {
        Ok((total, users)) => answer(
            StatusCode::OK,
            &list_resource(
                total,
                page.start(),
                users.iter().map(user_resource).collect(),
            ),
        ),
        Err(e) => broke("list the people", &e),
    }
}

/// The answer to a group route: whatever is in the database NOW, read back
/// rather than assembled from what was just sent.
async fn group_answer(state: &InternalState, id: &str, status: StatusCode) -> Response {
    match directory::get_group(&state.db, id).await {
        Ok(Some(group)) => answer(status, &group_resource(&group)),
        Ok(None) => not_found("group"),
        Err(e) => broke("read a group", &e),
    }
}

/// `None` if no OTHER group holds this name.
async fn holder_of(state: &InternalState, name: &str) -> anyhow::Result<Option<String>> {
    Ok(directory::group_by_name(&state.db, name)
        .await?
        .map(|g| g.id))
}

async fn create_group(State(state): State<InternalState>, body: Bytes) -> Response {
    let (id, name, members) = match parsed(&body).and_then(|raw| checked_group(raw, None)) {
        Ok(group) => group,
        Err(why) => return bad_request(&why),
    };
    // UPSERT BY NAME, because the name is what treff stores in an account's
    // groups and what `may_read` compares. A group that comes back under a
    // new provider id is the same group to everybody here.
    //
    // The client then holds the OLD id, and its own `diff` sees an
    // `externalId` that never becomes what it sent — so it writes a `PUT` on
    // every sync. That is why `PUT /Groups/{id}` takes the id from the PATH
    // and does not insist the body agree, the way a user's does: insisting
    // would turn a harmless repetition into a refusal that repeats forever.
    let id = match holder_of(&state, &name).await {
        Ok(held) => held.unwrap_or(id),
        Err(e) => return broke("look a group up by name", &e),
    };
    match directory::put_group(&state.db, &id, &name, members.as_deref()).await {
        Ok(()) => group_answer(&state, &id, StatusCode::CREATED).await,
        Err(e) => broke("store a group", &e),
    }
}

async fn write_group(
    State(state): State<InternalState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let (id, name, members) = match parsed(&body).and_then(|raw| checked_group(raw, Some(&id))) {
        Ok(group) => group,
        Err(why) => return bad_request(&why),
    };
    match holder_of(&state, &name).await {
        Ok(Some(other)) if other != id => {
            return refusal(
                StatusCode::CONFLICT,
                Some("uniqueness"),
                "another group already has this displayName",
            );
        }
        Ok(_) => {}
        Err(e) => return broke("look a group up by name", &e),
    }
    match directory::put_group(&state.db, &id, &name, members.as_deref()).await {
        Ok(()) => group_answer(&state, &id, StatusCode::OK).await,
        Err(e) => broke("store a group", &e),
    }
}

async fn read_group(State(state): State<InternalState>, Path(id): Path<String>) -> Response {
    let id = match as_id(Some(&id), "id in the path") {
        Ok(id) => id,
        Err(why) => return bad_request(&why),
    };
    group_answer(&state, &id, StatusCode::OK).await
}

async fn remove_group(State(state): State<InternalState>, Path(id): Path<String>) -> Response {
    let id = match as_id(Some(&id), "id in the path") {
        Ok(id) => id,
        Err(why) => return bad_request(&why),
    };
    match directory::delete_group(&state.db, &id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => not_found("group"),
        Err(e) => broke("remove a group", &e),
    }
}

async fn list_groups(State(state): State<InternalState>, Query(page): Query<Page>) -> Response {
    let by_name = match page.equals("displayName") {
        Ok(f) => f,
        Err(why) => return bad_request(&why),
    };
    match directory::list_groups(&state.db, by_name, page.start(), page.count()).await {
        Ok((total, groups)) => answer(
            StatusCode::OK,
            &list_resource(
                total,
                page.start(),
                groups.iter().map(group_resource).collect(),
            ),
        ),
        Err(e) => broke("list the groups", &e),
    }
}

/// One member change, as one of the three shapes means it.
struct Change {
    add: bool,
    subject: String,
}

/// `members[value eq "<id>"]` — the shape `_patch_remove_users` builds, which
/// carries the member in the PATH and no value at all.
fn member_in_path(path: &str) -> Option<&str> {
    path.trim()
        .strip_prefix("members[value eq \"")
        .and_then(|rest| rest.strip_suffix("\"]"))
}

fn checked_operation(op: OperationIn) -> Checked<Vec<Change>> {
    let kind = op.op.unwrap_or_default().trim().to_ascii_lowercase();
    let path = op.path.unwrap_or_default();
    let unknown = || {
        refused(
            "invalidPath",
            "only the three member operations are taken here: add on `members`, \
             remove on `members`, remove on `members[value eq \"…\"]`",
        )
    };

    if let Some(one) = member_in_path(&path) {
        if kind != "remove" {
            return Err(unknown());
        }
        if op.value.is_some_and(|v| !v.is_null()) {
            return Err(refused(
                "invalidValue",
                "a remove by filter carries no value",
            ));
        }
        return Ok(vec![Change {
            add: false,
            subject: as_id(Some(one), "member")?,
        }]);
    }

    if path.trim() != "members" {
        return Err(unknown());
    }
    let add = match kind.as_str() {
        "add" => true,
        "remove" => false,
        // `replace` on `members` would mean "these and no others", which the
        // client never sends and which PUT already says.
        _ => return Err(unknown()),
    };
    let members: Vec<MemberIn> = match op.value {
        Some(value) => serde_json::from_value(value)
            .map_err(|e| refused("invalidValue", format!("not a list of members: {e}")))?,
        None => return Err(refused("invalidValue", "this operation needs a value")),
    };
    if members.is_empty() {
        return Err(refused("invalidValue", "no member in the value"));
    }
    members
        .into_iter()
        .map(|m| {
            Ok(Change {
                add,
                subject: as_id(m.value.as_deref(), "member")?,
            })
        })
        .collect()
}

/// THE WHOLE REQUEST OR NONE OF IT. Every operation is understood before the
/// first one is applied — a chunk that was half applied and then refused
/// would leave a membership nobody asked for, and the client, having seen the
/// refusal, would never come back to correct it.
async fn patch_group(
    State(state): State<InternalState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let id = match as_id(Some(&id), "id in the path") {
        Ok(id) => id,
        Err(why) => return bad_request(&why),
    };
    let raw: PatchIn = match parsed(&body) {
        Ok(raw) => raw,
        Err(why) => return bad_request(&why),
    };
    let operations = raw.operations.unwrap_or_default();
    if operations.is_empty() {
        return bad_request(&refused("invalidValue", "no operation in the request"));
    }
    let changes = match operations
        .into_iter()
        .map(checked_operation)
        .collect::<Checked<Vec<Vec<Change>>>>()
    {
        Ok(changes) => changes.into_iter().flatten().collect::<Vec<Change>>(),
        Err(why) => return bad_request(&why),
    };

    match directory::get_group(&state.db, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return not_found("group"),
        Err(e) => return broke("read a group", &e),
    }
    for (add, subjects) in [
        (true, collected(&changes, true)),
        (false, collected(&changes, false)),
    ] {
        if subjects.is_empty() {
            continue;
        }
        let done = if add {
            directory::add_members(&state.db, &id, &subjects).await
        } else {
            directory::remove_members(&state.db, &id, &subjects).await
        };
        if let Err(e) = done {
            return broke("change a group's members", &e);
        }
    }
    group_answer(&state, &id, StatusCode::OK).await
}

fn collected(changes: &[Change], add: bool) -> Vec<String> {
    changes
        .iter()
        .filter(|c| c.add == add)
        .map(|c| c.subject.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_a_uuid_in_one_spelling() {
        assert_eq!(
            checked_id("5B1E0C1C-1111-4A4A-9B9B-000000000001").as_deref(),
            Some("5b1e0c1c-1111-4a4a-9b9b-000000000001")
        );
        for no in [
            "",
            "ada",
            "5b1e0c1c-1111-4a4a-9b9b",
            "5b1e0c1c-1111-4a4a-9b9b-0000000000012",
            "5b1e0c1c11114a4a9b9b000000000001",
            "5b1e0c1c-1111-4a4a-9b9b-00000000000g",
            "../../etc/passwd",
        ] {
            assert_eq!(checked_id(no), None, "{no:?}");
        }
    }

    #[test]
    fn the_primary_address_wins_and_an_empty_one_is_none() {
        let email = |raw: &str| primary_email(serde_json::from_str(raw).expect("json"));
        assert_eq!(
            email(r#"[{"value": "a@example.org"}, {"value": "b@example.org", "primary": true}]"#)
                .as_deref(),
            Some("b@example.org")
        );
        assert_eq!(
            email(r#"[{"value": "a@example.org"}]"#).as_deref(),
            Some("a@example.org")
        );
        assert_eq!(email(r#"[{"value": "  "}, {"type": "work"}]"#), None);
        assert_eq!(email("[]"), None);
    }

    #[test]
    fn the_filter_is_that_one_shape_or_nothing() {
        let page = |filter: Option<&str>| Page {
            start_index: None,
            count: None,
            filter: filter.map(String::from),
        };
        assert_eq!(
            page(Some(r#"userName eq "Konrad""#))
                .equals("userName")
                .ok()
                .flatten(),
            Some("Konrad")
        );
        assert_eq!(page(None).equals("userName").ok().flatten(), None);
        for no in [
            r#"userName co "Kon""#,
            r#"displayName eq "Konrad""#,
            "userName eq Konrad",
            "nonsense",
            "",
        ] {
            assert!(page(Some(no)).equals("userName").is_err(), "{no:?}");
        }
    }

    #[test]
    fn a_member_in_a_path_is_read_out_of_it() {
        assert_eq!(member_in_path(r#"members[value eq "abc"]"#), Some("abc"));
        for no in [
            "members",
            r#"members[displayName eq "abc"]"#,
            r#"members[value co "abc"]"#,
            r#"members[value eq "abc"#,
        ] {
            assert_eq!(member_in_path(no), None, "{no:?}");
        }
    }
}

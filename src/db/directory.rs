//! The directory: people and groups as the identity provider pushes them over
//! SCIM (plan-stage-6, ADR 0007).
//!
//! It writes the rows everything else already reads — `accounts` with handle,
//! address and `groups_json` — so mentions, the `@` list, mail and `may_read`
//! need no second source. A sign-in writes the same row; whoever writes last
//! wins, and both write the same thing.

use crate::db::Db;
use sqlx::Row;

/// A person as SCIM describes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    /// The SCIM id, which is the OIDC `sub` (the user's UUID).
    pub id: String,
    pub user_name: String,
    pub display_name: String,
    pub email: Option<String>,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub id: String,
    pub name: String,
    /// Subjects, sorted.
    pub members: Vec<String>,
}

/// Recomputes `groups_json` of these people from their memberships — the one
/// place that turns the directory into what `may_read` reads. An inactive
/// person reads nothing.
async fn recompute(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    subjects: &[String],
) -> anyhow::Result<()> {
    for subject in subjects {
        sqlx::query(
            "UPDATE accounts SET groups_json = CASE WHEN active = 1 THEN
                 (SELECT json_group_array(name) FROM
                    (SELECT g.name AS name FROM scim_members m
                       JOIN scim_groups g ON g.id = m.group_id
                      WHERE m.subject = ?1 ORDER BY g.name))
               ELSE '[]' END
              WHERE subject = ?1",
        )
        .bind(subject)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn members_of(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: &str,
) -> anyhow::Result<Vec<String>> {
    Ok(
        sqlx::query_scalar("SELECT subject FROM scim_members WHERE group_id = ? ORDER BY subject")
            .bind(id)
            .fetch_all(&mut **tx)
            .await?,
    )
}

/// Creates or replaces a person.
///
/// The handle is the user name through `auth::checked_handle`, and only if
/// nobody else holds it — the rule a sign-in follows (`Sessions::create`).
/// Inactive: no handle, no address, no groups; the row and its name stay.
pub async fn put_user(db: &Db, user: &User) -> anyhow::Result<()> {
    let handle = if user.active {
        crate::auth::checked_handle(&user.user_name)
    } else {
        None
    };
    let email = if user.active {
        user.email.clone()
    } else {
        None
    };
    let mut tx = db.pool().begin().await?;
    sqlx::query(
        "INSERT INTO accounts (subject, name, email, seen_at, handle, groups_json,
                               scim_user_name, active)
         VALUES (?1, ?2, ?3, 0,
                 (SELECT ?4 WHERE NOT EXISTS
                    (SELECT 1 FROM accounts WHERE handle = ?4 AND subject <> ?1)),
                 '[]', ?5, ?6)
         ON CONFLICT(subject) DO UPDATE SET name = excluded.name,
                                            email = excluded.email,
                                            handle = excluded.handle,
                                            scim_user_name = excluded.scim_user_name,
                                            active = excluded.active",
    )
    .bind(&user.id)
    .bind(&user.display_name)
    .bind(&email)
    .bind(&handle)
    .bind(&user.user_name)
    .bind(i64::from(user.active))
    .execute(&mut *tx)
    .await?;
    recompute(&mut tx, std::slice::from_ref(&user.id)).await?;
    tx.commit().await?;
    db.changed(crate::db::EVERY_SPACE);
    Ok(())
}

fn user_from(r: &sqlx::sqlite::SqliteRow) -> User {
    User {
        id: r.get("subject"),
        user_name: r.get("scim_user_name"),
        display_name: r.get("name"),
        email: r.get("email"),
        active: r.get::<i64, _>("active") != 0,
    }
}

pub async fn get_user(db: &Db, id: &str) -> anyhow::Result<Option<User>> {
    let row = sqlx::query(
        "SELECT subject, scim_user_name, name, email, active FROM accounts
          WHERE subject = ? AND scim_user_name IS NOT NULL",
    )
    .bind(id)
    .fetch_optional(db.pool())
    .await?;
    Ok(row.as_ref().map(user_from))
}

/// The people SCIM has sent, optionally by user name, a page at a time
/// (`start` is 1-based, as in SCIM). Returns the total and the page.
pub async fn list_users(
    db: &Db,
    user_name: Option<&str>,
    start: i64,
    count: i64,
) -> anyhow::Result<(i64, Vec<User>)> {
    let (offset, limit) = ((start.max(1)) - 1, count.clamp(0, 200));
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM accounts
          WHERE scim_user_name IS NOT NULL AND (?1 IS NULL OR scim_user_name = ?1)",
    )
    .bind(user_name)
    .fetch_one(db.pool())
    .await?;
    let rows = sqlx::query(
        "SELECT subject, scim_user_name, name, email, active FROM accounts
          WHERE scim_user_name IS NOT NULL AND (?1 IS NULL OR scim_user_name = ?1)
          ORDER BY subject LIMIT ?2 OFFSET ?3",
    )
    .bind(user_name)
    .bind(limit)
    .bind(offset)
    .fetch_all(db.pool())
    .await?;
    Ok((total, rows.iter().map(user_from).collect()))
}

/// Somebody left: like inactive, and out of every group. `false` if SCIM never
/// sent them.
pub async fn delete_user(db: &Db, id: &str) -> anyhow::Result<bool> {
    let Some(user) = get_user(db, id).await? else {
        return Ok(false);
    };
    put_user(
        db,
        &User {
            active: false,
            ..user
        },
    )
    .await?;
    sqlx::query("DELETE FROM scim_members WHERE subject = ?")
        .bind(id)
        .execute(db.pool())
        .await?;
    db.changed(crate::db::EVERY_SPACE);
    Ok(true)
}

/// Creates or renames a group; `members` replaces the members when given.
pub async fn put_group(
    db: &Db,
    id: &str,
    name: &str,
    members: Option<&[String]>,
) -> anyhow::Result<()> {
    let mut tx = db.pool().begin().await?;
    let before = members_of(&mut tx, id).await?;
    sqlx::query(
        "INSERT INTO scim_groups (id, name) VALUES (?, ?)
         ON CONFLICT(id) DO UPDATE SET name = excluded.name",
    )
    .bind(id)
    .bind(name)
    .execute(&mut *tx)
    .await?;
    if let Some(members) = members {
        sqlx::query("DELETE FROM scim_members WHERE group_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        for subject in members {
            sqlx::query("INSERT OR IGNORE INTO scim_members (group_id, subject) VALUES (?, ?)")
                .bind(id)
                .bind(subject)
                .execute(&mut *tx)
                .await?;
        }
    }
    // Everybody before and after: a rename moves them all, a replacement
    // moves whoever left as much as whoever came.
    let mut touched = before;
    touched.extend(members_of(&mut tx, id).await?);
    touched.sort();
    touched.dedup();
    recompute(&mut tx, &touched).await?;
    tx.commit().await?;
    db.changed(crate::db::EVERY_SPACE);
    Ok(())
}

/// By id or by name — two fixed statements, not one assembled from a column
/// name.
async fn group_where(db: &Db, by_name: bool, value: &str) -> anyhow::Result<Option<Group>> {
    let query = if by_name {
        sqlx::query("SELECT id, name FROM scim_groups WHERE name = ?")
    } else {
        sqlx::query("SELECT id, name FROM scim_groups WHERE id = ?")
    };
    let Some(row) = query.bind(value).fetch_optional(db.pool()).await? else {
        return Ok(None);
    };
    let id: String = row.get("id");
    let members =
        sqlx::query_scalar("SELECT subject FROM scim_members WHERE group_id = ? ORDER BY subject")
            .bind(&id)
            .fetch_all(db.pool())
            .await?;
    Ok(Some(Group {
        id,
        name: row.get("name"),
        members,
    }))
}

pub async fn get_group(db: &Db, id: &str) -> anyhow::Result<Option<Group>> {
    group_where(db, false, id).await
}

/// The group with this name, if any — for `POST` on a name that exists.
pub async fn group_by_name(db: &Db, name: &str) -> anyhow::Result<Option<Group>> {
    group_where(db, true, name).await
}

pub async fn list_groups(
    db: &Db,
    name: Option<&str>,
    start: i64,
    count: i64,
) -> anyhow::Result<(i64, Vec<Group>)> {
    let (offset, limit) = ((start.max(1)) - 1, count.clamp(0, 200));
    let total: i64 =
        sqlx::query_scalar("SELECT count(*) FROM scim_groups WHERE ?1 IS NULL OR name = ?1")
            .bind(name)
            .fetch_one(db.pool())
            .await?;
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM scim_groups WHERE ?1 IS NULL OR name = ?1 ORDER BY name LIMIT ?2 OFFSET ?3",
    )
    .bind(name)
    .bind(limit)
    .bind(offset)
    .fetch_all(db.pool())
    .await?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(g) = get_group(db, &id).await? {
            out.push(g);
        }
    }
    Ok((total, out))
}

async fn change_members(db: &Db, id: &str, subjects: &[String], add: bool) -> anyhow::Result<()> {
    let mut tx = db.pool().begin().await?;
    for subject in subjects {
        let sql = if add {
            "INSERT OR IGNORE INTO scim_members (group_id, subject) VALUES (?, ?)"
        } else {
            "DELETE FROM scim_members WHERE group_id = ? AND subject = ?"
        };
        sqlx::query(sql)
            .bind(id)
            .bind(subject)
            .execute(&mut *tx)
            .await?;
    }
    recompute(&mut tx, subjects).await?;
    tx.commit().await?;
    db.changed(crate::db::EVERY_SPACE);
    Ok(())
}

pub async fn add_members(db: &Db, id: &str, subjects: &[String]) -> anyhow::Result<()> {
    change_members(db, id, subjects, true).await
}

pub async fn remove_members(db: &Db, id: &str, subjects: &[String]) -> anyhow::Result<()> {
    change_members(db, id, subjects, false).await
}

pub async fn delete_group(db: &Db, id: &str) -> anyhow::Result<bool> {
    let mut tx = db.pool().begin().await?;
    let before = members_of(&mut tx, id).await?;
    let gone = sqlx::query("DELETE FROM scim_groups WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        == 1;
    recompute(&mut tx, &before).await?;
    tx.commit().await?;
    db.changed(crate::db::EVERY_SPACE);
    Ok(gone)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&dir.path().join("t.db")).await.expect("open");
        (dir, db)
    }

    const KONRAD: &str = "5b1e0c1c-1111-4a4a-9b9b-000000000001";
    const ADA: &str = "5b1e0c1c-1111-4a4a-9b9b-000000000002";

    fn konrad() -> User {
        User {
            id: KONRAD.into(),
            user_name: "Konrad".into(),
            display_name: "Konrad Müller".into(),
            email: Some("konrad@example.org".into()),
            active: true,
        }
    }

    async fn row(db: &Db, subject: &str) -> (Option<String>, Option<String>, String, String) {
        let r =
            sqlx::query("SELECT handle, email, groups_json, name FROM accounts WHERE subject = ?")
                .bind(subject)
                .fetch_one(db.pool())
                .await
                .expect("row");
        (
            r.get("handle"),
            r.get("email"),
            r.get("groups_json"),
            r.get("name"),
        )
    }

    /// A PERSON WHO NEVER SIGNED IN is known, reachable and — once in a group
    /// that may read — mentionable. That is the whole point of the stage.
    #[tokio::test]
    async fn a_person_from_scim_becomes_a_reader_through_their_group() {
        let (_d, db) = db().await;
        put_user(&db, &konrad()).await.expect("put");
        let (handle, email, groups, name) = row(&db, KONRAD).await;
        assert_eq!(handle.as_deref(), Some("konrad"));
        assert_eq!(email.as_deref(), Some("konrad@example.org"));
        assert_eq!(groups, "[]", "no group yet");
        assert_eq!(name, "Konrad Müller");

        put_group(&db, "g-1", "Freunde", None).await.expect("group");
        add_members(&db, "g-1", &[KONRAD.to_string()])
            .await
            .expect("add");
        assert_eq!(row(&db, KONRAD).await.2, r#"["Freunde"]"#);

        let got = get_group(&db, "g-1").await.expect("get").expect("there");
        assert_eq!(got.members, vec![KONRAD.to_string()]);
        remove_members(&db, "g-1", &[KONRAD.to_string()])
            .await
            .expect("remove");
        assert_eq!(row(&db, KONRAD).await.2, "[]");
    }

    #[tokio::test]
    async fn a_put_with_members_replaces_them_and_moves_everybody_touched() {
        let (_d, db) = db().await;
        put_user(&db, &konrad()).await.expect("konrad");
        put_user(
            &db,
            &User {
                id: ADA.into(),
                user_name: "ada".into(),
                ..konrad()
            },
        )
        .await
        .expect("ada");
        put_group(&db, "g-1", "Freunde", Some(&[KONRAD.to_string()]))
            .await
            .expect("one");
        put_group(&db, "g-1", "Freunde", Some(&[ADA.to_string()]))
            .await
            .expect("other");
        assert_eq!(row(&db, KONRAD).await.2, "[]", "the one who was replaced");
        assert_eq!(row(&db, ADA).await.2, r#"["Freunde"]"#);
    }

    /// A membership may arrive before the person; it is there when they come.
    #[tokio::test]
    async fn a_membership_before_the_person_counts_when_they_arrive() {
        let (_d, db) = db().await;
        put_group(&db, "g-1", "Freunde", Some(&[KONRAD.to_string()]))
            .await
            .expect("group");
        put_user(&db, &konrad()).await.expect("put");
        assert_eq!(row(&db, KONRAD).await.2, r#"["Freunde"]"#);
    }

    /// Two people, one user name: the second gets no handle.
    #[tokio::test]
    async fn a_taken_handle_is_not_given_twice() {
        let (_d, db) = db().await;
        put_user(&db, &konrad()).await.expect("konrad");
        put_user(
            &db,
            &User {
                id: ADA.into(),
                ..konrad()
            },
        )
        .await
        .expect("ada");
        assert_eq!(row(&db, ADA).await.0, None);
        assert_eq!(row(&db, KONRAD).await.0.as_deref(), Some("konrad"));
    }

    /// LEAVING CLEARS, IT DOES NOT ERASE. The row stays for the posts.
    #[tokio::test]
    async fn inactive_and_deleted_strip_the_row_and_keep_it() {
        let (_d, db) = db().await;
        put_user(&db, &konrad()).await.expect("put");
        put_group(&db, "g-1", "Freunde", Some(&[KONRAD.to_string()]))
            .await
            .expect("group");
        put_user(
            &db,
            &User {
                active: false,
                ..konrad()
            },
        )
        .await
        .expect("off");
        assert_eq!(
            row(&db, KONRAD).await,
            (None, None, "[]".into(), "Konrad Müller".into())
        );

        put_user(&db, &konrad()).await.expect("back");
        assert_eq!(
            row(&db, KONRAD).await.2,
            r#"["Freunde"]"#,
            "back in the group it never left"
        );

        assert!(delete_user(&db, KONRAD).await.expect("delete"));
        assert_eq!(row(&db, KONRAD).await.0, None);
        assert!(
            get_group(&db, "g-1")
                .await
                .expect("get")
                .expect("there")
                .members
                .is_empty()
        );
        assert!(!delete_user(&db, "never-sent").await.expect("delete"));
    }

    /// The sign-in writes the same row, not a second one.
    #[tokio::test]
    async fn a_sign_in_after_scim_is_the_same_account() {
        let (_d, db) = db().await;
        put_user(&db, &konrad()).await.expect("put");
        crate::auth::Sessions::create(
            &db,
            &crate::authz::Identity {
                subject: KONRAD.into(),
                name: "Konrad Müller".into(),
                groups: vec!["Freunde".into()],
                email: Some("konrad@example.org".into()),
                handle: Some("konrad".into()),
            },
        )
        .await
        .expect("sign-in");
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM accounts")
            .fetch_one(db.pool())
            .await
            .expect("count");
        assert_eq!(n, 1);
        assert_eq!(row(&db, KONRAD).await.0.as_deref(), Some("konrad"));
    }

    #[tokio::test]
    async fn lists_find_by_name_and_page() {
        let (_d, db) = db().await;
        put_user(&db, &konrad()).await.expect("konrad");
        put_user(
            &db,
            &User {
                id: ADA.into(),
                user_name: "ada".into(),
                ..konrad()
            },
        )
        .await
        .expect("ada");
        let (total, page) = list_users(&db, None, 1, 1).await.expect("list");
        assert_eq!((total, page.len()), (2, 1));
        let (total, page) = list_users(&db, Some("Konrad"), 1, 10)
            .await
            .expect("filter");
        assert_eq!(total, 1);
        assert_eq!(page[0].id, KONRAD);
        put_group(&db, "g-1", "Freunde", None).await.expect("group");
        let (total, _) = list_groups(&db, Some("Freunde"), 1, 10)
            .await
            .expect("groups");
        assert_eq!(total, 1);
        assert_eq!(
            group_by_name(&db, "Freunde")
                .await
                .expect("by name")
                .map(|g| g.id),
            Some("g-1".into())
        );
        assert!(delete_group(&db, "g-1").await.expect("delete"));
    }
}

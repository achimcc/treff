//! Who may do what. This module knows **no HTTP and no database** — which is
//! exactly why the rules can be tested exhaustively here instead of seeping
//! into handlers, where every route would decide them again slightly
//! differently.

use crate::config::{Category, Space};

#[derive(Debug, Clone)]
pub struct Identity {
    /// The `sub` claim. *It* is the person, not the name — names change, and
    /// matching on one hands other people's posts to a namesake.
    pub subject: String,
    pub name: String,
    pub groups: Vec<String>,
}

impl Identity {
    /// True if this identity is in at least one of `allowed`. An empty
    /// `allowed` therefore grants nothing, which is the whole point.
    pub fn in_any(&self, allowed: &[String]) -> bool {
        allowed.iter().any(|g| self.groups.contains(g))
    }
}

pub fn may_read(id: &Identity, space: &Space) -> bool {
    id.in_any(&space.read)
}

pub fn may_post(id: &Identity, cat: &Category) -> bool {
    id.in_any(&cat.post)
}

pub fn may_reply(id: &Identity, cat: &Category) -> bool {
    id.in_any(&cat.reply)
}

/// Everyone edits and deletes their own, and nobody else's — including
/// whoever runs the instance. Stage 1 has no moderation layer on purpose
/// (design, §5).
pub fn may_modify(id: &Identity, author_subject: &str) -> bool {
    id.subject == author_subject
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Category, Space, View};

    fn who(groups: &[&str]) -> Identity {
        Identity {
            subject: "sub-1".into(),
            name: "Someone".into(),
            groups: groups.iter().map(|g| (*g).to_string()).collect(),
        }
    }

    fn category_with(post: &[&str], reply: &[&str]) -> Category {
        Category {
            slug: "k".into(),
            title: "K".into(),
            post: post.iter().map(|g| (*g).to_string()).collect(),
            reply: reply.iter().map(|g| (*g).to_string()).collect(),
        }
    }

    fn space_read_by(read: &[&str]) -> Space {
        Space {
            host: "h".into(),
            title: "H".into(),
            view: View::Topics,
            read: read.iter().map(|g| (*g).to_string()).collect(),
            categories: vec![],
            articles: None,
            attachment_max_bytes: 8 * 1024 * 1024,
            title_key: "title".into(),
        }
    }

    #[test]
    fn reading_needs_one_of_the_spaces_groups() {
        assert!(may_read(
            &who(&["Household"]),
            &space_read_by(&["Household", "Friends"])
        ));
        assert!(!may_read(
            &who(&["Strangers"]),
            &space_read_by(&["Household"])
        ));
    }

    #[test]
    fn posting_and_replying_are_separate_rights() {
        let k = category_with(&["Writers"], &["Household"]);
        let member = who(&["Household"]);
        assert!(
            !may_post(&member, &k),
            "a mere member must not open a topic here"
        );
        assert!(may_reply(&member, &k));
    }

    #[test]
    fn an_empty_group_list_grants_nothing() {
        // Fail closed: an empty list is NOT "everybody", it is "nobody".
        let k = category_with(&[], &[]);
        assert!(!may_post(&who(&["Household"]), &k));
        assert!(!may_reply(&who(&["Household"]), &k));
        assert!(!may_read(&who(&["Household"]), &space_read_by(&[])));
    }

    #[test]
    fn someone_with_no_groups_may_nothing() {
        let stranger = who(&[]);
        assert!(!may_read(&stranger, &space_read_by(&["Household"])));
        assert!(!may_post(
            &stranger,
            &category_with(&["Household"], &["Household"])
        ));
        assert!(!may_reply(
            &stranger,
            &category_with(&["Household"], &["Household"])
        ));
    }

    #[test]
    fn only_the_author_may_modify() {
        let me = who(&["Household"]);
        assert!(may_modify(&me, "sub-1"));
        assert!(!may_modify(&me, "sub-2"));
    }

    #[test]
    fn no_group_is_an_administrator() {
        // Stage 1 has no moderation. Even someone in every group does not
        // touch other people's posts (design, §5).
        let omnipotent = who(&["Household", "Friends", "Writers", "Operations"]);
        assert!(!may_modify(&omnipotent, "sub-2"));
    }

    #[test]
    fn group_names_are_compared_exactly() {
        // No lower-casing, no trimming: the identity provider is the source,
        // and a fuzzy comparison would be privilege escalation by typo.
        assert!(!may_read(
            &who(&["household"]),
            &space_read_by(&["Household"])
        ));
        assert!(!may_read(
            &who(&[" Household"]),
            &space_read_by(&["Household"])
        ));
    }

    #[test]
    fn the_subject_decides_authorship_not_the_name() {
        // Two people may share a display name, and a person may change theirs.
        // Matching on the name would hand over someone else's posts.
        let renamed = Identity {
            subject: "sub-1".into(),
            name: "A Different Name".into(),
            groups: vec![],
        };
        assert!(may_modify(&renamed, "sub-1"));

        let namesake = Identity {
            subject: "sub-9".into(),
            name: "Someone".into(),
            groups: vec![],
        };
        assert!(!may_modify(&namesake, "sub-1"));
    }
}

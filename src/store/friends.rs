//! Friends, friend requests and blocked people: the data behind the Friends
//! view, shared by the GUI and the terminal.

use std::collections::HashMap;

use super::Store;
use crate::discord::member_list::{activity_line, Status};
use crate::discord::models::{PresenceUpdate, RelationKind, Relationship, User};

/// Most relationships kept. Discord allows 1000 friends; blocked people and
/// requests add some more. Anything beyond is ignored (and logged).
pub const MAX_RELATIONS: usize = 5000;

/// The tabs of the Friends view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FriendsTab {
    #[default]
    Online,
    All,
    /// Requests received, then sent.
    Pending,
    Blocked,
    /// The field to send a friend request by username.
    Add,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relation {
    pub kind: RelationKind,
    /// The nickname we gave them (only we see it).
    pub nickname: Option<String>,
}

/// A friend's status, as shown in the list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FriendPresence {
    pub status: Status,
    /// Custom status or game (already cleaned and shortened).
    pub activity: Option<String>,
}

#[derive(Debug, Default)]
pub struct Friends {
    relations: HashMap<String, Relation>,
    /// The IDs of `relations` sorted by name: rebuilt when a relation
    /// changes, so the view never sorts.
    order: Vec<String>,
    /// Only for friends.
    presence: HashMap<String, FriendPresence>,
}

impl Store {
    /// READY: all relationships and our friends' status, replacing the old ones.
    pub(super) fn set_relationships(&mut self, rels: Vec<Relationship>, presences: &[PresenceUpdate]) {
        self.friends = Friends::default();
        for r in rels {
            self.insert_relation(r);
        }
        for p in presences {
            self.set_presence(p);
        }
        self.sort_relations();
    }

    /// `RELATIONSHIP_ADD` / `_UPDATE`, or our own action that succeeded.
    pub fn upsert_relationship(&mut self, r: Relationship) {
        let id = r.id.clone();
        let was_friend = self.relation(&id) == Some(RelationKind::Friend);
        if self.insert_relation(r) {
            if was_friend && self.relation(&id) != Some(RelationKind::Friend) {
                self.friends.presence.remove(&id);
            }
            self.sort_relations();
        }
    }

    /// `RELATIONSHIP_REMOVE`, or our own removal that succeeded.
    pub fn remove_relationship(&mut self, id: &str) {
        if self.friends.relations.remove(id).is_some() {
            self.friends.presence.remove(id);
            self.friends.order.retain(|x| x != id);
        }
    }

    /// `PRESENCE_UPDATE` outside servers: kept only for friends.
    pub fn apply_presence(&mut self, p: &PresenceUpdate) {
        self.set_presence(p);
    }

    /// What this person is to us.
    pub fn relation(&self, user_id: &str) -> Option<RelationKind> {
        self.friends.relations.get(user_id).map(|r| r.kind)
    }

    /// Friend requests waiting for our answer.
    pub fn incoming_requests(&self) -> usize {
        self.friends
            .relations
            .values()
            .filter(|r| r.kind == RelationKind::Incoming)
            .count()
    }

    pub fn friend_presence(&self, user_id: &str) -> FriendPresence {
        self.friends.presence.get(user_id).cloned().unwrap_or_default()
    }

    /// The people of a tab, sorted by name (the requests received before the
    /// sent ones).
    pub fn friends_in(&self, tab: FriendsTab) -> Vec<&str> {
        let of = |kind: RelationKind| {
            self.friends
                .order
                .iter()
                .filter(move |id| self.relation(id) == Some(kind))
                .map(String::as_str)
        };
        match tab {
            FriendsTab::All => of(RelationKind::Friend).collect(),
            FriendsTab::Online => of(RelationKind::Friend)
                .filter(|id| self.friend_presence(id).status != Status::Offline)
                .collect(),
            FriendsTab::Pending => of(RelationKind::Incoming)
                .chain(of(RelationKind::Outgoing))
                .collect(),
            FriendsTab::Blocked => of(RelationKind::Blocked).collect(),
            FriendsTab::Add => Vec::new(),
        }
    }

    /// The name to show: our nickname for them, else their display name.
    pub fn friend_name(&self, user_id: &str) -> String {
        if let Some(n) = self
            .friends
            .relations
            .get(user_id)
            .and_then(|r| r.nickname.as_deref())
            .filter(|n| !n.trim().is_empty())
        {
            return n.to_string();
        }
        self.users.get(user_id).map_or_else(
            || crate::i18n::t("Unknown user", "Utente sconosciuto").into(),
            |u| u.display_name().to_string(),
        )
    }

    /// Stores one relation; `false` if nothing was stored.
    fn insert_relation(&mut self, r: Relationship) -> bool {
        if !crate::media::valid_id(&r.id) {
            return false;
        }
        let Some(kind) = RelationKind::from_code(r.kind) else {
            // A kind we do not show (or a missing one): never a friend.
            if self.friends.relations.remove(&r.id).is_some() {
                self.friends.presence.remove(&r.id);
                return true;
            }
            return false;
        };
        if !self.friends.relations.contains_key(&r.id) && self.friends.relations.len() >= MAX_RELATIONS {
            tracing::warn!("friends: more than {MAX_RELATIONS} relationships, the others are ignored");
            return false;
        }
        if let Some(u) = r.user.filter(|u| u.id == r.id) {
            self.remember_user(u);
        }
        let nickname = r.nickname.filter(|n| !n.is_empty());
        let nickname = match (nickname, self.friends.relations.get(&r.id)) {
            // `RELATIONSHIP_UPDATE` may leave the nickname out: keep ours.
            (None, Some(old)) if old.kind == kind => old.nickname.clone(),
            (n, _) => n,
        };
        self.friends.relations.insert(r.id, Relation { kind, nickname });
        true
    }

    fn remember_user(&mut self, u: User) {
        // A partial user (no name) never replaces a complete one.
        if u.username.is_empty() && self.users.contains_key(&u.id) {
            return;
        }
        self.users.insert(u.id.clone(), u);
    }

    fn set_presence(&mut self, p: &PresenceUpdate) {
        if p.guild_id.is_some() || self.relation(&p.user.id) != Some(RelationKind::Friend) {
            return;
        }
        let presence = FriendPresence {
            status: Status::parse(&p.status),
            activity: activity_line(&p.activities),
        };
        if presence == FriendPresence::default() {
            self.friends.presence.remove(&p.user.id);
        } else {
            self.friends.presence.insert(p.user.id.clone(), presence);
        }
    }

    fn sort_relations(&mut self) {
        let mut keyed: Vec<(String, String)> = self
            .friends
            .relations
            .keys()
            .map(|id| (self.friend_name(id).to_lowercase(), id.clone()))
            .collect();
        keyed.sort();
        self.friends.order = keyed.into_iter().map(|(_, id)| id).collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(id: &str, kind: u8, name: Option<&str>) -> Relationship {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "type": kind,
            "user": name.map(|n| serde_json::json!({"id": id, "username": n})),
        }))
        .unwrap()
    }

    fn presence(id: &str, status: &str, guild: Option<&str>) -> PresenceUpdate {
        serde_json::from_value(serde_json::json!({
            "user": {"id": id}, "status": status, "guild_id": guild,
            "activities": [{"type": 0, "name": "Chess"}],
        }))
        .unwrap()
    }

    #[test]
    fn tabs_sorted_and_filtered() {
        let mut s = Store::default();
        s.set_relationships(
            vec![
                rel("10", 1, Some("zed")),
                rel("11", 1, Some("ada")),
                rel("12", 3, Some("carl")),
                rel("13", 4, Some("bea")),
                rel("14", 2, Some("eve")),
                rel("15", 5, Some("implicit")),
                rel("not-an-id", 1, Some("bad")),
            ],
            &[presence("10", "online", None), presence("12", "online", None)],
        );
        assert_eq!(s.friends_in(FriendsTab::All), ["11", "10"]);
        assert_eq!(s.friends_in(FriendsTab::Online), ["10"]);
        // Received first, then sent.
        assert_eq!(s.friends_in(FriendsTab::Pending), ["12", "13"]);
        assert_eq!(s.friends_in(FriendsTab::Blocked), ["14"]);
        assert_eq!(s.incoming_requests(), 1);
        assert_eq!(s.relation("15"), None);
        // A request's status is not kept: only friends'.
        assert_eq!(s.friend_presence("12"), FriendPresence::default());
        assert_eq!(s.friend_presence("10").activity.as_deref(), Some("Playing Chess"));
    }

    #[test]
    fn changes_from_the_gateway() {
        let mut s = Store::default();
        s.set_relationships(vec![rel("12", 3, Some("carl"))], &[]);
        // A request accepted: a friend now, without user in the event.
        s.upsert_relationship(rel("12", 1, None));
        assert_eq!(s.relation("12"), Some(RelationKind::Friend));
        assert_eq!(s.friend_name("12"), "carl");
        // Presences inside a server are ignored.
        s.apply_presence(&presence("12", "dnd", Some("99")));
        assert_eq!(s.friend_presence("12").status, Status::Offline);
        s.apply_presence(&presence("12", "dnd", None));
        assert_eq!(s.friend_presence("12").status, Status::DoNotDisturb);
        // Blocked: no status kept.
        s.upsert_relationship(rel("12", 2, None));
        assert_eq!(s.friend_presence("12"), FriendPresence::default());
        s.remove_relationship("12");
        assert_eq!(s.relation("12"), None);
        assert!(s.friends_in(FriendsTab::Blocked).is_empty());
        // An unknown kind never makes a friend.
        s.upsert_relationship(rel("20", 1, Some("x")));
        s.upsert_relationship(rel("20", 9, None));
        assert_eq!(s.relation("20"), None);
    }

    #[test]
    fn nickname_and_limit() {
        let mut s = Store::default();
        let mut r = rel("10", 1, Some("zed"));
        r.nickname = Some("Zio".into());
        s.set_relationships(vec![r], &[]);
        assert_eq!(s.friend_name("10"), "Zio");
        // An update without the nickname keeps it.
        s.upsert_relationship(rel("10", 1, None));
        assert_eq!(s.friend_name("10"), "Zio");
        let many: Vec<_> = (0..MAX_RELATIONS + 10)
            .map(|i| rel(&format!("{}", 1000 + i), 1, None))
            .collect();
        s.set_relationships(many, &[]);
        assert_eq!(s.friends_in(FriendsTab::All).len(), MAX_RELATIONS);
    }
}

//! The member list on the right of a server channel.
//!
//! As Discord's own client gets it: we subscribe to a channel's list with ranges of rows
//! (op 37) and Discord answers with `GUILD_MEMBER_LIST_UPDATE` events
//! that sync, insert, update, delete or invalidate rows.
//!
//! The list is flat: group headers (a hoisted role, "online", "offline")
//! and members, in the order shown. Indexes from the network are checked
//! and the list is bounded: a broken or hostile event can never panic or
//! grow it without limit.

use serde::Deserialize;

use super::models::{GuildMember, User};

/// Rows asked per range (Discord's own unit).
pub const PAGE: usize = 100;
/// At most this many pages are asked (the first ones).
pub const MAX_PAGES: usize = 5;
/// Rows kept at most.
pub const MAX_ROWS: usize = PAGE * MAX_PAGES;

/// `GUILD_MEMBER_LIST_UPDATE`.
#[derive(Debug, Clone, Deserialize)]
pub struct MemberListUpdate {
    pub guild_id: String,
    /// The list: channels with the same visibility share it.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub member_count: u64,
    #[serde(default)]
    pub online_count: u64,
    #[serde(default)]
    pub groups: Vec<GroupInfo>,
    #[serde(default)]
    pub ops: Vec<ListOp>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroupInfo {
    pub id: String,
    #[serde(default)]
    pub count: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "UPPERCASE")]
pub enum ListOp {
    Sync {
        range: [usize; 2],
        #[serde(default)]
        items: Vec<RawItem>,
    },
    Insert {
        index: usize,
        item: RawItem,
    },
    Update {
        index: usize,
        item: RawItem,
    },
    Delete {
        index: usize,
    },
    Invalidate {
        range: [usize; 2],
    },
    /// Anything new Discord may add: ignored.
    #[serde(other)]
    Unknown,
}

/// A row as it arrives: a group header or a member.
#[derive(Debug, Clone, Deserialize)]
pub struct RawItem {
    #[serde(default)]
    pub group: Option<GroupInfo>,
    #[serde(default)]
    pub member: Option<ListMember>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListMember {
    #[serde(flatten)]
    pub member: GuildMember,
    #[serde(default)]
    pub presence: Option<Presence>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Presence {
    #[serde(default)]
    pub status: String,
    /// Custom status and games; Discord may leave them out.
    #[serde(default)]
    pub activities: Vec<Activity>,
}

/// One activity of a presence: only what the list shows.
#[derive(Debug, Clone, Deserialize)]
pub struct Activity {
    /// 0 playing, 1 streaming, 2 listening, 3 watching, 4 custom status, 5 competing.
    #[serde(rename = "type", default)]
    pub kind: u8,
    #[serde(default)]
    pub name: String,
    /// The text of a custom status.
    #[serde(default)]
    pub state: Option<String>,
}

/// Longest activity text kept (Discord's custom status limit).
pub const MAX_ACTIVITY_CHARS: usize = 128;

/// The line to show under a name: the custom status first, otherwise what
/// they are playing, listening to or watching. The text comes from other
/// people: control characters go and its length is limited.
pub fn activity_line(activities: &[Activity]) -> Option<String> {
    use crate::i18n::t;
    let custom = activities
        .iter()
        .find(|a| a.kind == 4)
        .and_then(|a| a.state.as_deref())
        .map(clean)
        .filter(|s| !s.is_empty());
    if custom.is_some() {
        return custom;
    }
    let a = activities.iter().find(|a| matches!(a.kind, 0 | 2 | 3 | 5))?;
    let name = clean(&a.name);
    if name.is_empty() {
        return None;
    }
    let verb = match a.kind {
        0 => t("Playing", "Gioca a"),
        2 => t("Listening to", "Ascolta"),
        3 => t("Watching", "Guarda"),
        _ => t("Competing in", "Gareggia in"),
    };
    Some(format!("{verb} {name}"))
}

fn clean(s: &str) -> String {
    let one_line: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_ACTIVITY_CHARS)
        .collect();
    one_line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Online status shown with the avatar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    Online,
    Idle,
    DoNotDisturb,
    #[default]
    Offline,
}

impl Status {
    pub fn parse(s: &str) -> Self {
        match s {
            "online" => Self::Online,
            "idle" => Self::Idle,
            "dnd" => Self::DoNotDisturb,
            _ => Self::Offline,
        }
    }
}

/// A row of the list, ready to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// Group header: role ID, `online` or `offline`, and how many are in it.
    Group { id: String, count: u64 },
    Member {
        user_id: String,
        status: Status,
        /// Custom status or game, see [`activity_line`].
        activity: Option<String>,
    },
    /// Not known (invalidated, or not asked).
    Unknown,
}

/// The list of one server, for the channel we subscribed to.
#[derive(Debug, Clone, Default)]
pub struct MemberList {
    pub guild_id: String,
    /// The list ID Discord gave (empty until the first event).
    pub id: String,
    pub member_count: u64,
    pub online_count: u64,
    pub rows: Vec<Row>,
}

impl MemberList {
    pub fn new(guild_id: &str) -> Self {
        Self {
            guild_id: guild_id.to_string(),
            ..Default::default()
        }
    }

    /// Applies an event; the members it carries are given to `remember`
    /// (user, server profile) so names and avatars are known everywhere.
    pub fn apply(&mut self, u: MemberListUpdate, mut remember: impl FnMut(User, GuildMember)) {
        if u.guild_id != self.guild_id {
            return;
        }
        if self.id != u.id {
            // Another list (another channel, or the visibility changed):
            // start again.
            self.id = u.id;
            self.rows.clear();
        }
        self.member_count = u.member_count;
        self.online_count = u.online_count;
        let mut convert = |item: RawItem| -> Row {
            if let Some(g) = item.group {
                return Row::Group {
                    id: g.id,
                    count: g.count,
                };
            }
            let Some(m) = item.member else { return Row::Unknown };
            let status = Status::parse(m.presence.as_ref().map_or("", |p| p.status.as_str()));
            let activity = m.presence.as_ref().and_then(|p| activity_line(&p.activities));
            let mut member = m.member;
            let Some(user) = member.user.take() else {
                return Row::Unknown;
            };
            let user_id = user.id.clone();
            member.user_id = Some(user_id.clone());
            remember(user, member);
            Row::Member {
                user_id,
                status,
                activity,
            }
        };
        for op in u.ops {
            match op {
                ListOp::Sync { range, items } => {
                    let [start, end] = range;
                    if start > end || start >= MAX_ROWS {
                        continue;
                    }
                    let end = end.min(MAX_ROWS - 1);
                    if self.rows.len() <= end {
                        self.rows.resize(end + 1, Row::Unknown);
                    }
                    for (i, item) in (start..=end).zip(items) {
                        self.rows[i] = convert(item);
                    }
                }
                ListOp::Insert { index, item } => {
                    if index <= self.rows.len() && index < MAX_ROWS {
                        self.rows.insert(index, convert(item));
                        self.rows.truncate(MAX_ROWS);
                    }
                }
                ListOp::Update { index, item } => {
                    if let Some(row) = self.rows.get_mut(index) {
                        *row = convert(item);
                    }
                }
                ListOp::Delete { index } => {
                    if index < self.rows.len() {
                        self.rows.remove(index);
                    }
                }
                ListOp::Invalidate { range } => {
                    let [start, end] = range;
                    for row in self.rows.iter_mut().take(end.saturating_add(1)).skip(start) {
                        *row = Row::Unknown;
                    }
                }
                ListOp::Unknown => {}
            }
        }
        // The counts are in `groups`: the group rows often come without.
        for row in &mut self.rows {
            if let Row::Group { id, count } = row {
                if let Some(g) = u.groups.iter().find(|g| g.id == *id) {
                    *count = g.count;
                }
            }
        }
        // Unknown rows at the end are of no use.
        while self.rows.last() == Some(&Row::Unknown) {
            self.rows.pop();
        }
    }

    /// Rows that are known, in order.
    pub fn known(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(|r| **r != Row::Unknown)
    }
}

/// Ranges to ask for the first `pages` pages: `[[0, 99], [100, 199], …]`.
pub fn ranges(pages: usize) -> Vec<[usize; 2]> {
    (0..pages.clamp(1, MAX_PAGES))
        .map(|p| [p * PAGE, p * PAGE + PAGE - 1])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(id: &str, status: &str) -> serde_json::Value {
        serde_json::json!({"member": {
            "user": {"id": id, "username": format!("user{id}")},
            "roles": [], "nick": null,
            "presence": {"status": status, "user": {"id": id}}
        }})
    }

    fn update(ops: serde_json::Value) -> MemberListUpdate {
        serde_json::from_value(serde_json::json!({
            "guild_id": "100", "id": "everyone", "member_count": 3, "online_count": 2,
            "groups": [{"id": "online", "count": 2}, {"id": "offline", "count": 1}],
            "ops": ops
        }))
        .unwrap()
    }

    #[test]
    fn sync_insert_update_delete() {
        let mut list = MemberList::new("100");
        let mut seen = Vec::new();
        list.apply(
            update(serde_json::json!([{"op": "SYNC", "range": [0, 99], "items": [
                {"group": {"id": "online"}},
                member("901", "online"),
                member("902", "dnd"),
                {"group": {"id": "offline", "count": 1}},
                member("903", "offline"),
            ]}])),
            |u, _| seen.push(u.id),
        );
        assert_eq!(seen, ["901", "902", "903"]);
        // The count comes from `groups` when the row has none.
        assert_eq!(
            list.rows[0],
            Row::Group {
                id: "online".into(),
                count: 2
            }
        );
        assert_eq!(list.rows.len(), 5);
        assert_eq!(
            list.rows[2],
            Row::Member {
                user_id: "902".into(),
                status: Status::DoNotDisturb,
                activity: None,
            }
        );
        list.apply(
            update(serde_json::json!([
                {"op": "UPDATE", "index": 1, "item": member("901", "idle")},
                {"op": "INSERT", "index": 3, "item": member("904", "online")},
                {"op": "DELETE", "index": 2},
                {"op": "SOMETHING_NEW"},
            ])),
            |_, _| {},
        );
        let ids: Vec<&str> = list
            .known()
            .filter_map(|r| match r {
                Row::Member { user_id, .. } => Some(user_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, ["901", "904", "903"]);
        assert_eq!(
            list.rows[1],
            Row::Member {
                user_id: "901".into(),
                status: Status::Idle,
                activity: None,
            }
        );
    }

    #[test]
    fn activity_lines() {
        let acts = |v: serde_json::Value| -> Vec<Activity> { serde_json::from_value(v).unwrap() };
        // The custom status comes first, cleaned of control characters.
        assert_eq!(
            activity_line(&acts(serde_json::json!([
                {"type": 0, "name": "Factorio"},
                {"type": 4, "name": "Custom Status", "state": "Back \u{001b}[31m soon\n!"}
            ]))),
            Some("Back [31m soon !".into())
        );
        assert_eq!(
            activity_line(&acts(serde_json::json!([{"type": 0, "name": "Factorio"}]))),
            Some("Playing Factorio".into())
        );
        // Streaming (1) is shown from the voice state, not here; empty texts are nothing.
        assert_eq!(
            activity_line(&acts(serde_json::json!([{"type": 1, "name": "x"}]))),
            None
        );
        assert_eq!(
            activity_line(&acts(serde_json::json!([{"type": 4, "state": "  "}]))),
            None
        );
        assert_eq!(activity_line(&[]), None);
        // At most 128 characters.
        let long = "x".repeat(500);
        let line = activity_line(&acts(serde_json::json!([{"type": 4, "state": long}]))).unwrap();
        assert_eq!(line.chars().count(), MAX_ACTIVITY_CHARS);
    }

    #[test]
    fn hostile_indexes_never_panic_nor_grow_the_list() {
        let mut list = MemberList::new("100");
        list.apply(
            update(serde_json::json!([
                {"op": "UPDATE", "index": 50, "item": member("901", "online")},
                {"op": "DELETE", "index": 7},
                {"op": "INSERT", "index": 9, "item": member("901", "online")},
                {"op": "SYNC", "range": [99999999, 3], "items": []},
                {"op": "SYNC", "range": [0, 18446744073709551615u64], "items": [member("901", "online")]},
                {"op": "INVALIDATE", "range": [0, 18446744073709551615u64]},
            ])),
            |_, _| {},
        );
        assert!(list.rows.len() <= MAX_ROWS);
        // Another server's event is ignored.
        let mut other =
            update(serde_json::json!([{"op": "SYNC", "range": [0, 0], "items": [member("9", "online")]}]));
        other.guild_id = "200".into();
        list.apply(other, |_, _| panic!("not this server"));
    }

    #[test]
    fn a_new_list_starts_again() {
        let mut list = MemberList::new("100");
        list.apply(
            update(serde_json::json!([{"op": "SYNC", "range": [0, 1], "items": [member("901", "online"), member("902", "online")]}])),
            |_, _| {},
        );
        let mut u =
            update(serde_json::json!([{"op": "SYNC", "range": [0, 0], "items": [member("905", "online")]}]));
        u.id = "123456".into();
        list.apply(u, |_, _| {});
        assert_eq!(list.rows.len(), 1);
        assert_eq!(ranges(2), vec![[0, 99], [100, 199]]);
        assert_eq!(ranges(99).len(), MAX_PAGES);
    }
}

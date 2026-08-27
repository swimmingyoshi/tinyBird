//! Shared rooms, so friends can see what each other are playing.
//!
//! **Nothing here emulates anything.** The game runs in each player's own
//! browser; this server only relays small JSON messages between them. A room of
//! four people costs the same as a room of one, because the work is happening
//! on four different machines.
//!
//! What travels is the addon snapshot the overlay already renders — the same
//! shape `/api/snapshot` serves and the stream overlay consumes — so a member's
//! game can be displayed with the renderer that exists rather than a new one.
//!
//! A room is identified by a short code someone reads out loud. There is no
//! persistence: the last person to leave takes the room with them, which is the
//! right lifetime for something you open to play for an evening.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// How many messages a slow member may fall behind before being dropped.
///
/// Snapshots are worth little once they are stale, so the buffer is short: a
/// member who cannot keep up should skip ahead rather than replay a backlog.
const CHANNEL_CAPACITY: usize = 32;
/// Length of a room code.
const CODE_LEN: usize = 5;
/// Characters a room code is built from.
///
/// No vowels, so a random code cannot spell something unfortunate, and no
/// `0`/`O` or `1`/`I`, because these get read out over voice chat.
const CODE_ALPHABET: &[u8] = b"BCDFGHJKLMNPQRSTVWXYZ23456789";
/// The longest message a member may publish.
///
/// Sized for a picture of a Game Boy Advance screen, which is the largest thing
/// that legitimately travels: 240x160 encodes to something like ten kilobytes,
/// and base64 adds a third. The cap is well above that and well below anything
/// that would let a room be used as a file transfer.
pub const MAX_MESSAGE_BYTES: usize = 128 * 1024;
/// The longest display name a member may claim.
pub const MAX_NAME_LEN: usize = 24;
/// How many people may be in one room.
///
/// A room is a group of friends, not a broadcast. The cap is here because a
/// public deployment has no other limit on who joins a code.
pub const MAX_MEMBERS: usize = 8;
/// How many rooms may be open at once.
///
/// Rooms cost almost nothing, but "almost nothing" times unbounded is still
/// unbounded, and this server is reachable from the internet.
pub const MAX_ROOMS: usize = 200;

/// How long a room outlives the last person in it.
///
/// Not zero, because a dropped connection should not destroy the room somebody
/// is about to reconnect to — which is the common case behind a proxy that
/// closes idle sockets. Not long, because an abandoned room is just a code
/// nobody can reuse.
pub const EMPTY_ROOM_GRACE: Duration = Duration::from_secs(5 * 60);

/// Why a room could not be created or joined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinError {
    /// No room has that code.
    ///
    /// Distinct from the others because it is the one a person causes by
    /// mistyping, and the only useful answer is "check the code".
    NoSuchRoom,
    /// The room is full.
    RoomFull,
    /// The host has closed the room to new arrivals.
    Locked,
    /// This server is already hosting as many rooms as it will.
    TooManyRooms,
}

impl JoinError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::NoSuchRoom => "No room with that code. Check it and try again.",
            Self::RoomFull => "That room is full.",
            Self::Locked => "That room is not taking new players.",
            Self::TooManyRooms => "This server has too many rooms open. Try again shortly.",
        }
    }
}

/// Who is joining.
///
/// `user` is the account's stable subject when the server has accounts
/// configured, and `None` for a server running without them. It is what marks
/// the host, and it never comes from the browser — the same rule that governs
/// who owns a save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub user: Option<String>,
    pub name: String,
}

impl Identity {
    /// Someone signed in.
    pub fn account(user: &str, name: &str) -> Self {
        Self {
            user: Some(user.to_string()),
            name: clean_name(name),
        }
    }

    /// Someone on a server with no accounts. Their name is their own word for
    /// it, which is fine when the only person who can reach the server is the
    /// person running it.
    pub fn guest(name: &str) -> Self {
        Self {
            user: None,
            name: clean_name(name),
        }
    }
}

/// Someone in a room.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Member {
    /// Unique within the room, and what messages are addressed to.
    ///
    /// Per connection, not per person: the same account in two tabs is two
    /// members, which is what you want when one of them is a stream view.
    pub id: String,
    pub name: String,
    /// Whether this member opened the room.
    pub host: bool,
    /// The account behind this member, when there is one. Never serialized:
    /// the room needs it to recognise the host, the other members do not.
    #[serde(skip)]
    pub user: Option<String>,
    /// What they are playing, or `None` if nothing is loaded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playing: Option<String>,
    /// Cartridge code of that game, for showing the right icon.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_code: Option<String>,
    /// Which end of the link cable this member is, if any.
    ///
    /// A cable takes four consoles and a room takes eight, so the later
    /// arrivals have no seat and watch instead. Assigned by the server rather
    /// than worked out from the member list, because that list is sorted by
    /// name: two people with the same name, or one of them renaming, would
    /// otherwise silently swap seats in the middle of a trade.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seat: Option<u8>,
}

/// What the server sends to a room.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Outgoing {
    /// Sent to one member when they arrive: who they are and who else is here.
    Welcome {
        room: String,
        you: String,
        members: Vec<Member>,
    },
    /// The membership changed.
    Members { members: Vec<Member> },
    /// Someone's game state. `snapshot` is passed through untouched.
    Snapshot {
        from: String,
        snapshot: serde_json::Value,
    },
    /// A picture of someone's screen, as a data URL.
    ///
    /// Separate from the snapshot because they answer different questions and
    /// travel at different rates: a read-out is worth sending twice a second,
    /// a picture ten times, and only while somebody is looking.
    Frame { from: String, frame: String },
    /// How far the parent has got, in frames since the cable was plugged in.
    ///
    /// The barrier that keeps the two consoles on the same frame. A child may
    /// run up to this and no further.
    LinkTick { frame: u32 },
    /// The parent has started a link transfer and wants everyone's halfword.
    /// `frame` plus `offset` identifies the same point within a linked frame on
    /// consoles whose absolute cycle counters and attachment times differ.
    LinkStart { seq: u32, frame: u32, offset: u32 },
    /// One console's halfword, on its way to the parent.
    LinkValue { from: String, seq: u32, value: u16 },
    /// What every console sent, in seat order, and how long the cable takes
    /// to clock it. From the parent only.
    LinkData {
        seq: u32,
        values: [u16; 4],
        cycles: u32,
    },
    /// The room opened or closed to new arrivals.
    Locked { locked: bool },
    /// Something the sender did was refused.
    Error { message: String },
}

/// What a member sends.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Incoming {
    /// Update what this member is shown as playing.
    Playing {
        #[serde(default)]
        playing: Option<String>,
        #[serde(default)]
        game_code: Option<String>,
    },
    /// Publish a snapshot to everyone else.
    Snapshot { snapshot: serde_json::Value },
    /// Publish a picture of this member's screen.
    Frame { frame: String },
    /// Say how far the parent has got. Host only: it is the clock.
    LinkTick { frame: u32 },
    /// Announce a link transfer. Host only: the parent drives the cable.
    LinkStart {
        seq: u32,
        #[serde(default)]
        frame: u32,
        #[serde(default)]
        offset: u32,
    },
    /// Offer this console's halfword for the transfer in progress.
    LinkValue { seq: u32, value: u16 },
    /// Publish what every console sent, with the parent's transfer time.
    /// Host only.
    LinkData {
        seq: u32,
        values: [u16; 4],
        #[serde(default)]
        cycles: u32,
    },
    /// Close the room to new arrivals, or open it again. Host only.
    Lock { locked: bool },
}

/// One room.
#[derive(Debug)]
struct Room {
    /// The account that opened it, if the server has accounts. `None` means
    /// the first person through the door becomes host.
    host: Option<String>,
    members: HashMap<String, Member>,
    sender: broadcast::Sender<String>,
    /// Closed to new arrivals.
    locked: bool,
    /// When the room last became empty, for the grace period.
    empty_since: Option<Instant>,
}

/// Every room on this server.
#[derive(Debug, Default)]
pub struct Lobby {
    rooms: Mutex<HashMap<String, Room>>,
}

/// How many consoles one link cable joins.
///
/// The same limit the hardware has. Room members beyond this are spectators.
pub const MAX_LINK_SEATS: u8 = 4;

/// A member's handle on the room they joined.
pub struct Joined {
    pub room: String,
    pub member_id: String,
    pub members: Vec<Member>,
    pub receiver: broadcast::Receiver<String>,
}

#[cfg(test)]
impl Joined {
    /// The member with this name, for tests that care about one of them.
    fn iter_member(&self, name: &str) -> &Member {
        self.members
            .iter()
            .find(|m| m.name == name)
            .unwrap_or_else(|| panic!("no member called {name}"))
    }
}

/// A random room code.
pub fn new_room_code() -> String {
    let mut bytes = [0u8; CODE_LEN];
    // Room codes are not secrets, but a predictable one lets a stranger guess
    // their way into a room, so they come from OS entropy rather than a clock.
    getrandom::getrandom(&mut bytes).expect("the OS must provide randomness");
    bytes
        .iter()
        .map(|b| CODE_ALPHABET[*b as usize % CODE_ALPHABET.len()] as char)
        .collect()
}

/// A random member id.
fn new_member_id() -> String {
    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes).expect("the OS must provide randomness");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Reduce a room code to the canonical form used as a key.
///
/// Codes get read out and typed back in, so case and stray spaces should not
/// decide whether two people end up in the same room.
pub fn normalise_code(code: &str) -> String {
    code.trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .take(16)
        .collect()
}

/// Trim a display name to something safe to show.
///
/// Names are rendered as text by every other member, so control characters and
/// unbounded length are somebody else's problem unless they are stopped here.
pub fn clean_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME_LEN)
        .collect();
    if cleaned.is_empty() {
        "guest".to_string()
    } else {
        cleaned
    }
}

impl Lobby {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a room, and return its code.
    ///
    /// Rooms exist only because somebody made one. An earlier version created
    /// a room for any code that arrived, which meant a mistyped code silently
    /// opened an empty room instead of saying the code was wrong — and made
    /// every typo count against the server's room limit.
    pub fn create(&self, host: Option<&str>) -> Result<String, JoinError> {
        let mut rooms = self.rooms.lock().unwrap_or_else(|err| err.into_inner());
        Self::sweep(&mut rooms, Instant::now());

        if rooms.len() >= MAX_ROOMS {
            return Err(JoinError::TooManyRooms);
        }

        // A collision would hand someone else's room to whoever asked next.
        let code = std::iter::repeat_with(new_room_code)
            .take(8)
            .find(|code| !rooms.contains_key(code))
            .ok_or(JoinError::TooManyRooms)?;

        rooms.insert(
            code.clone(),
            Room {
                host: host.map(|h| h.to_string()),
                members: HashMap::new(),
                sender: broadcast::channel(CHANNEL_CAPACITY).0,
                locked: false,
                // Created and not yet joined: the grace period starts now, so
                // a room nobody ever enters does not sit here forever.
                empty_since: Some(Instant::now()),
            },
        );
        Ok(code)
    }

    /// Join an existing room.
    pub fn join(&self, code: &str, who: &Identity) -> Result<Joined, JoinError> {
        let room_code = normalise_code(code);

        let mut rooms = self.rooms.lock().unwrap_or_else(|err| err.into_inner());
        Self::sweep(&mut rooms, Instant::now());

        let room = rooms.get_mut(&room_code).ok_or(JoinError::NoSuchRoom)?;
        if room.members.len() >= MAX_MEMBERS {
            return Err(JoinError::RoomFull);
        }
        // The host is always let back in; a lock is for strangers, and locking
        // yourself out by reconnecting would be a trap.
        let is_host = match (&room.host, &who.user) {
            (Some(host), Some(user)) => host == user,
            // A server without accounts gives the room to whoever arrives
            // first, which is the person who just created it.
            (None, _) => room.members.is_empty(),
            _ => false,
        };
        if room.locked && !is_host {
            return Err(JoinError::Locked);
        }

        if room.host.is_none() && is_host {
            room.host = who.user.clone();
        }

        let member = Member {
            id: new_member_id(),
            name: who.name.clone(),
            host: is_host,
            user: who.user.clone(),
            playing: None,
            game_code: None,
            seat: free_seat(&room.members, is_host),
        };

        let receiver = room.sender.subscribe();
        room.members.insert(member.id.clone(), member.clone());
        room.empty_since = None;
        let members = sorted(&room.members);

        Ok(Joined {
            room: room_code,
            member_id: member.id,
            members,
            receiver,
        })
    }

    /// Whether this member belongs to the account that opened the room.
    ///
    /// More than one tab can have this authority. Cable ownership is narrower
    /// and is checked separately by [`Self::is_link_parent`].
    #[cfg(test)]
    pub fn is_host(&self, room: &str, member_id: &str) -> bool {
        let rooms = self.rooms.lock().unwrap_or_else(|err| err.into_inner());
        rooms
            .get(room)
            .and_then(|entry| entry.members.get(member_id))
            .is_some_and(|member| member.host)
    }

    /// Whether this member is the one console allowed to drive the cable.
    ///
    /// An account owner may have the room open in more than one tab. Every one
    /// of those tabs is allowed to administer the room, but only the tab in
    /// seat zero is the link parent. Keeping this separate from room authority
    /// prevents two host tabs from both clocking the same cable.
    pub fn is_link_parent(&self, room: &str, member_id: &str) -> bool {
        let rooms = self.rooms.lock().unwrap_or_else(|err| err.into_inner());
        rooms
            .get(room)
            .and_then(|entry| entry.members.get(member_id))
            .is_some_and(|member| member.seat == Some(0))
    }

    /// Close a room to new arrivals, or open it again. Host only.
    pub fn set_locked(&self, room: &str, member_id: &str, locked: bool) -> Option<bool> {
        let mut rooms = self.rooms.lock().unwrap_or_else(|err| err.into_inner());
        let entry = rooms.get_mut(room)?;
        if !entry.members.get(member_id)?.host {
            return None;
        }
        entry.locked = locked;
        Some(locked)
    }

    /// Whether a room is closed to new arrivals.
    ///
    /// The lock reaches members over the socket, so nothing on the request path
    /// asks; this is here for the tests that check a refused lock changed
    /// nothing.
    #[cfg(test)]
    pub fn is_locked(&self, room: &str) -> bool {
        self.rooms
            .lock()
            .ok()
            .and_then(|rooms| rooms.get(room).map(|r| r.locked))
            .unwrap_or(false)
    }

    /// Run the sweep at a given instant. For tests, which cannot wait minutes.
    #[cfg(test)]
    pub fn sweep_at(&self, now: Instant) {
        let mut rooms = self.rooms.lock().unwrap_or_else(|err| err.into_inner());
        Self::sweep(&mut rooms, now);
    }

    /// Drop rooms that have been empty longer than the grace period.
    ///
    /// Lazy, on create and join, rather than a background task: rooms only
    /// matter when somebody is asking about one.
    fn sweep(rooms: &mut HashMap<String, Room>, now: Instant) {
        rooms.retain(|_, room| match room.empty_since {
            Some(since) => now.duration_since(since) < EMPTY_ROOM_GRACE,
            None => true,
        });
    }

    /// Remove a member.
    ///
    /// An empty room is kept for [`EMPTY_ROOM_GRACE`] rather than destroyed,
    /// so that a dropped connection does not take the room with it while its
    /// only occupant is reconnecting.
    pub fn leave(&self, room: &str, member_id: &str) -> Option<Vec<Member>> {
        let mut rooms = self.rooms.lock().unwrap_or_else(|err| err.into_inner());
        let entry = rooms.get_mut(room)?;
        entry.members.remove(member_id)?;

        if entry.members.is_empty() {
            entry.empty_since = Some(Instant::now());
            return Some(Vec::new());
        }
        Some(sorted(&entry.members))
    }

    /// Record what a member is playing, returning the new membership.
    pub fn set_playing(
        &self,
        room: &str,
        member_id: &str,
        playing: Option<String>,
        game_code: Option<String>,
    ) -> Option<Vec<Member>> {
        let mut rooms = self.rooms.lock().unwrap_or_else(|err| err.into_inner());
        let entry = rooms.get_mut(room)?;
        let member = entry.members.get_mut(member_id)?;
        member.playing = playing.map(|value| clean_name(&value));
        member.game_code = game_code.map(|value| clean_name(&value));
        Some(sorted(&entry.members))
    }

    /// Send a message to everyone in a room.
    ///
    /// Failure means nobody is listening, which is not an error worth raising:
    /// a member publishing into an empty room is an ordinary state.
    pub fn broadcast(&self, room: &str, message: &Outgoing) {
        let Ok(text) = serde_json::to_string(message) else {
            return;
        };
        let rooms = self.rooms.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(entry) = rooms.get(room) {
            let _ = entry.sender.send(text);
        }
    }

    /// How many rooms are open. For the health endpoint.
    pub fn room_count(&self) -> usize {
        self.rooms
            .lock()
            .map(|rooms| rooms.len())
            .unwrap_or_default()
    }
}

/// Members in a stable order, so the list does not shuffle between updates.
/// The lowest seat on the cable nobody is sitting in.
///
/// Seat 0 is the host's and nobody else's: it is the parent, the console that
/// clocks the cable. Everyone else takes the lowest free seat, so somebody
/// leaving frees theirs for the next arrival rather than stranding it.
fn free_seat(members: &HashMap<String, Member>, is_host: bool) -> Option<u8> {
    let taken: Vec<u8> = members.values().filter_map(|member| member.seat).collect();
    if is_host && !taken.contains(&0) {
        return Some(0);
    }
    (1..MAX_LINK_SEATS).find(|seat| !taken.contains(seat))
}

fn sorted(members: &HashMap<String, Member>) -> Vec<Member> {
    let mut list: Vec<Member> = members.values().cloned().collect();
    list.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A signed-in member. The name is the only thing they choose.
    fn user(id: &str, name: &str) -> Identity {
        Identity::account(id, name)
    }

    /// Open a room and return its code.
    fn room_hosted_by(lobby: &Lobby, host: &Identity) -> String {
        lobby.create(host.user.as_deref()).expect("should open")
    }

    #[test]
    fn a_room_code_is_readable_over_voice() {
        let code = new_room_code();
        assert_eq!(code.len(), CODE_LEN);
        for c in code.chars() {
            assert!(CODE_ALPHABET.contains(&(c as u8)), "unexpected {c}");
            assert!(!"OI01AEU".contains(c), "{c} is easy to mishear");
        }
    }

    #[test]
    fn room_codes_differ() {
        assert_ne!(new_room_code(), new_room_code());
    }

    #[test]
    fn codes_are_matched_regardless_of_how_they_were_typed() {
        assert_eq!(normalise_code(" bc-df "), "BCDF");
        assert_eq!(normalise_code("BCDF"), normalise_code("bcdf"));
        assert_eq!(normalise_code("b c d f"), "BCDF");
    }

    /// The rule the whole design turns on.
    ///
    /// An earlier version created a room for any code that arrived, so a
    /// mistyped code opened an empty room instead of saying the code was
    /// wrong — and every typo counted against the server's room limit.
    #[test]
    fn a_code_nobody_created_is_not_a_room() {
        let lobby = Lobby::new();
        assert_eq!(
            lobby.join("NEVERMADE", &user("u1", "Ada")).err(),
            Some(JoinError::NoSuchRoom)
        );
        assert_eq!(lobby.room_count(), 0, "a failed join must not open a room");
    }

    #[test]
    fn a_created_room_can_be_joined_however_the_code_is_typed() {
        let lobby = Lobby::new();
        let ada = user("u1", "Ada");
        let code = room_hosted_by(&lobby, &ada);

        let lower = code.to_lowercase();
        assert!(lobby.join(&lower, &user("u2", "Bea")).is_ok());
    }

    #[test]
    fn the_person_who_opened_the_room_is_its_host() {
        let lobby = Lobby::new();
        let ada = user("u1", "Ada");
        let code = room_hosted_by(&lobby, &ada);

        let joined = lobby.join(&code, &ada).expect("host can join");
        assert!(joined.members[0].host, "the creator should be host");

        let bea = lobby.join(&code, &user("u2", "Bea")).expect("should join");
        let bea_member = bea
            .members
            .iter()
            .find(|m| m.name == "Bea")
            .expect("Bea is listed");
        assert!(!bea_member.host, "a guest is not the host");
    }

    /// The same account in two tabs is two members, one room, one host.
    #[test]
    fn the_host_is_still_the_host_in_a_second_tab() {
        let lobby = Lobby::new();
        let ada = user("u1", "Ada");
        let code = room_hosted_by(&lobby, &ada);

        lobby.join(&code, &ada).expect("first tab");
        let second = lobby.join(&code, &ada).expect("second tab");
        assert!(
            second.members.iter().all(|m| m.host),
            "both of the host's connections are the host"
        );
    }

    #[test]
    fn a_locked_room_turns_strangers_away() {
        let lobby = Lobby::new();
        let ada = user("u1", "Ada");
        let code = room_hosted_by(&lobby, &ada);
        let host = lobby.join(&code, &ada).expect("host joins");

        assert_eq!(lobby.set_locked(&code, &host.member_id, true), Some(true));
        assert_eq!(
            lobby.join(&code, &user("u2", "Bea")).err(),
            Some(JoinError::Locked)
        );

        lobby.set_locked(&code, &host.member_id, false);
        assert!(lobby.join(&code, &user("u2", "Bea")).is_ok());
    }

    /// Locking must not lock the host out of their own room on a reconnect.
    #[test]
    fn a_locked_room_still_admits_its_host() {
        let lobby = Lobby::new();
        let ada = user("u1", "Ada");
        let code = room_hosted_by(&lobby, &ada);
        let host = lobby.join(&code, &ada).expect("host joins");
        lobby.set_locked(&code, &host.member_id, true);

        assert!(lobby.join(&code, &ada).is_ok(), "the host comes back in");
    }

    #[test]
    fn only_the_host_may_lock_the_room() {
        let lobby = Lobby::new();
        let ada = user("u1", "Ada");
        let code = room_hosted_by(&lobby, &ada);
        lobby.join(&code, &ada).expect("host joins");
        let bea = lobby.join(&code, &user("u2", "Bea")).expect("Bea joins");

        assert_eq!(lobby.set_locked(&code, &bea.member_id, true), None);
        assert!(!lobby.is_locked(&code), "the room must still be open");
    }

    /// Without accounts the server is one person's, so the first in hosts.
    #[test]
    fn a_server_without_accounts_gives_the_room_to_whoever_arrives_first() {
        let lobby = Lobby::new();
        let code = lobby.create(None).expect("opens");

        let first = lobby.join(&code, &Identity::guest("Ada")).expect("joins");
        assert!(first.members[0].host);

        let second = lobby.join(&code, &Identity::guest("Bea")).expect("joins");
        let bea = second.iter_member("Bea");
        assert!(!bea.host);
    }

    #[test]
    fn members_are_listed_in_a_stable_order() {
        let lobby = Lobby::new();
        let code = lobby.create(None).expect("opens");
        lobby.join(&code, &Identity::guest("Zoe")).unwrap();
        lobby.join(&code, &Identity::guest("Ada")).unwrap();
        let third = lobby.join(&code, &Identity::guest("Mel")).unwrap();

        let names: Vec<&str> = third.members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["Ada", "Mel", "Zoe"]);
    }

    /// A dropped connection must not take the room with it.
    #[test]
    fn an_empty_room_survives_long_enough_to_reconnect_to() {
        let lobby = Lobby::new();
        let ada = user("u1", "Ada");
        let code = room_hosted_by(&lobby, &ada);
        let joined = lobby.join(&code, &ada).expect("joins");

        lobby.leave(&code, &joined.member_id);
        assert_eq!(lobby.room_count(), 1, "the room should still be there");
        assert!(lobby.join(&code, &ada).is_ok(), "and still be joinable");
    }

    #[test]
    fn an_abandoned_room_is_eventually_swept_up() {
        let lobby = Lobby::new();
        let code = lobby.create(Some("u1")).expect("opens");
        assert_eq!(lobby.room_count(), 1);

        lobby.sweep_at(Instant::now() + EMPTY_ROOM_GRACE + Duration::from_secs(1));
        assert_eq!(lobby.room_count(), 0, "an empty room should not last forever");
        assert_eq!(
            lobby.join(&code, &user("u1", "Ada")).err(),
            Some(JoinError::NoSuchRoom)
        );
    }

    #[test]
    fn a_room_with_people_in_it_is_never_swept() {
        let lobby = Lobby::new();
        let ada = user("u1", "Ada");
        let code = room_hosted_by(&lobby, &ada);
        lobby.join(&code, &ada).expect("joins");

        lobby.sweep_at(Instant::now() + EMPTY_ROOM_GRACE * 10);
        assert_eq!(lobby.room_count(), 1);
    }

    #[test]
    fn leaving_twice_is_harmless() {
        let lobby = Lobby::new();
        let code = lobby.create(None).expect("opens");
        let a = lobby.join(&code, &Identity::guest("Ada")).unwrap();
        assert!(lobby.leave(&code, &a.member_id).is_some());
        assert!(lobby.leave(&code, &a.member_id).is_none());
    }

    #[test]
    fn what_someone_is_playing_shows_up_in_the_list() {
        let lobby = Lobby::new();
        let code = lobby.create(None).expect("opens");
        let a = lobby.join(&code, &Identity::guest("Ada")).unwrap();

        let members = lobby
            .set_playing(&code, &a.member_id, Some("Minish Cap".into()), Some("BZME".into()))
            .expect("member exists");
        assert_eq!(members[0].playing.as_deref(), Some("Minish Cap"));
        assert_eq!(members[0].game_code.as_deref(), Some("BZME"));
    }

    #[test]
    fn a_member_of_another_room_cannot_be_touched() {
        let lobby = Lobby::new();
        let one = lobby.create(None).expect("opens");
        let two = lobby.create(None).expect("opens");
        let a = lobby.join(&one, &Identity::guest("Ada")).unwrap();

        assert!(lobby.set_playing(&two, &a.member_id, None, None).is_none());
        assert!(lobby.leave(&two, &a.member_id).is_none());
        assert!(lobby.set_locked(&two, &a.member_id, true).is_none());
    }

    #[test]
    fn names_are_cleaned_before_anyone_else_sees_them() {
        assert_eq!(clean_name("  Ada  "), "Ada");
        assert_eq!(clean_name(""), "guest");
        assert_eq!(clean_name("a\u{0}b\nc"), "abc", "control characters go");
        assert_eq!(clean_name(&"x".repeat(100)).len(), MAX_NAME_LEN);
        // And through the identity, which is what the room actually stores.
        assert_eq!(Identity::guest("  Ada  ").name, "Ada");
        assert_eq!(Identity::account("u1", "a\nb").name, "ab");
    }

    /// The account behind a member is the room's business, not the room-mates'.
    #[test]
    fn a_members_account_is_not_broadcast() {
        let member = Member {
            id: "m1".into(),
            name: "Ada".into(),
            host: true,
            user: Some("secret-subject-claim".into()),
            playing: None,
            game_code: None,
            seat: Some(0),
        };
        let json = serde_json::to_string(&member).unwrap();
        assert!(!json.contains("secret-subject-claim"), "got {json}");
        assert!(json.contains("\"host\":true"));
    }

    /// Which seat each member ended up in, by name.
    fn seats(members: &[Member]) -> Vec<(String, Option<u8>)> {
        let mut list: Vec<(String, Option<u8>)> = members
            .iter()
            .map(|m| (m.name.clone(), m.seat))
            .collect();
        list.sort();
        list
    }

    #[test]
    fn the_host_takes_the_parent_seat_and_everyone_else_queues_behind() {
        let lobby = Lobby::new();
        let code = lobby.create(Some("ada")).expect("opens");

        let host = lobby.join(&code, &Identity::account("ada", "Ada")).expect("host joins");
        assert_eq!(host.members[0].seat, Some(0), "the host is the parent");

        for (user, name) in [("bea", "Bea"), ("cal", "Cal"), ("dot", "Dot")] {
            lobby
                .join(&code, &Identity::account(user, name))
                .expect("joins");
        }
        let members = lobby
            .join(&code, &Identity::account("eve", "Eve"))
            .expect("joins")
            .members;

        // Four seats on a cable; the fifth arrival watches instead.
        assert_eq!(
            seats(&members),
            vec![
                ("Ada".to_string(), Some(0)),
                ("Bea".to_string(), Some(1)),
                ("Cal".to_string(), Some(2)),
                ("Dot".to_string(), Some(3)),
                ("Eve".to_string(), None),
            ]
        );
    }

    #[test]
    fn a_seat_is_freed_when_its_console_leaves() {
        let lobby = Lobby::new();
        let code = lobby.create(Some("ada")).expect("opens");
        lobby.join(&code, &Identity::account("ada", "Ada")).expect("host");
        let bea = lobby.join(&code, &Identity::account("bea", "Bea")).expect("joins");
        lobby.join(&code, &Identity::account("cal", "Cal")).expect("joins");

        assert_eq!(bea.members.iter().find(|m| m.name == "Bea").unwrap().seat, Some(1));

        lobby.leave(&code, &bea.member_id).expect("leaves");

        // Seat 1 is free again, so the next arrival takes it rather than
        // being stranded without one while a seat sits empty.
        let dot = lobby.join(&code, &Identity::account("dot", "Dot")).expect("joins");
        assert_eq!(
            dot.members.iter().find(|m| m.name == "Dot").unwrap().seat,
            Some(1)
        );
    }

    /// The parent is the only console that may clock the cable, so the server
    /// has to know who that is rather than believing whoever claims it.
    #[test]
    fn only_the_host_is_the_host() {
        let lobby = Lobby::new();
        let code = lobby.create(Some("ada")).expect("opens");
        let host = lobby.join(&code, &Identity::account("ada", "Ada")).expect("host");
        let guest = lobby.join(&code, &Identity::account("bea", "Bea")).expect("joins");

        assert!(lobby.is_host(&code, &host.member_id));
        assert!(!lobby.is_host(&code, &guest.member_id));
        // And nothing is the host of a room that does not exist.
        assert!(!lobby.is_host("ZZZZZ", &host.member_id));
        assert!(!lobby.is_host(&code, "not-a-member"));
    }

    #[test]
    fn a_second_host_tab_is_a_child_not_a_second_link_parent() {
        let lobby = Lobby::new();
        let code = lobby.create(Some("ada")).expect("opens");
        let first = lobby
            .join(&code, &Identity::account("ada", "Ada one"))
            .expect("joins");
        let second = lobby
            .join(&code, &Identity::account("ada", "Ada two"))
            .expect("joins");

        assert!(lobby.is_host(&code, &first.member_id));
        assert!(lobby.is_host(&code, &second.member_id));
        assert!(lobby.is_link_parent(&code, &first.member_id));
        assert!(!lobby.is_link_parent(&code, &second.member_id));
        assert_eq!(first.iter_member("Ada one").seat, Some(0));
        assert_eq!(second.iter_member("Ada two").seat, Some(1));
    }

    #[test]
    fn a_room_only_holds_so_many_people() {
        let lobby = Lobby::new();
        let code = lobby.create(None).expect("opens");
        for i in 0..MAX_MEMBERS {
            assert!(
                lobby.join(&code, &Identity::guest(&format!("p{i}"))).is_ok(),
                "member {i}"
            );
        }
        assert_eq!(
            lobby.join(&code, &Identity::guest("late")).err(),
            Some(JoinError::RoomFull)
        );
    }

    #[test]
    fn the_server_stops_opening_new_rooms_eventually() {
        let lobby = Lobby::new();
        for i in 0..MAX_ROOMS {
            assert!(lobby.create(None).is_ok(), "room {i}");
        }
        assert_eq!(lobby.create(None).err(), Some(JoinError::TooManyRooms));
    }

    #[test]
    fn a_subscriber_receives_what_is_broadcast() {
        let lobby = Lobby::new();
        let code = lobby.create(None).expect("opens");
        let mut a = lobby.join(&code, &Identity::guest("Ada")).unwrap();

        lobby.broadcast(&code, &Outgoing::Members { members: Vec::new() });
        let text = a.receiver.try_recv().expect("should have a message");
        assert!(text.contains("\"type\":\"members\""), "got {text}");
    }

    #[test]
    fn broadcasting_into_a_room_that_is_gone_is_not_an_error() {
        Lobby::new().broadcast("nobody-here", &Outgoing::Members { members: Vec::new() });
    }

    #[test]
    fn messages_serialize_with_a_type_the_page_can_switch_on() {
        let welcome = serde_json::to_string(&Outgoing::Welcome {
            room: "BCDFG".into(),
            you: "abc".into(),
            members: Vec::new(),
        })
        .unwrap();
        assert!(welcome.contains("\"type\":\"welcome\""), "got {welcome}");

        let locked = serde_json::to_string(&Outgoing::Locked { locked: true }).unwrap();
        assert!(locked.contains("\"type\":\"locked\""), "got {locked}");

        let snapshot = serde_json::to_string(&Outgoing::Snapshot {
            from: "abc".into(),
            snapshot: serde_json::json!({ "rom": { "game_code": "BPRE" } }),
        })
        .unwrap();
        assert!(snapshot.contains("BPRE"), "the snapshot must pass through");
    }

    #[test]
    fn incoming_messages_parse() {
        let playing: Incoming =
            serde_json::from_str(r#"{"type":"playing","playing":"Zelda","game_code":"BZME"}"#)
                .expect("should parse");
        assert!(matches!(playing, Incoming::Playing { .. }));

        let cleared: Incoming = serde_json::from_str(r#"{"type":"playing"}"#).expect("parses");
        assert!(matches!(
            cleared,
            Incoming::Playing { playing: None, game_code: None }
        ));

        let lock: Incoming =
            serde_json::from_str(r#"{"type":"lock","locked":true}"#).expect("parses");
        assert!(matches!(lock, Incoming::Lock { locked: true }));

        let snapshot: Incoming =
            serde_json::from_str(r#"{"type":"snapshot","snapshot":{"a":1}}"#).expect("parses");
        assert!(matches!(snapshot, Incoming::Snapshot { .. }));

        let start: Incoming = serde_json::from_str(
            r#"{"type":"link_start","seq":7,"frame":12,"offset":3456}"#,
        )
        .expect("parses");
        assert!(matches!(
            start,
            Incoming::LinkStart { seq: 7, frame: 12, offset: 3456 }
        ));
    }

    #[test]
    fn an_unknown_message_is_rejected_rather_than_guessed_at() {
        assert!(serde_json::from_str::<Incoming>(r#"{"type":"whatever"}"#).is_err());
    }
}

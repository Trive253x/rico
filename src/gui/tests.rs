//! Tests of the GUI, mostly in the demo (no network, no account).

use super::*;

fn demo() -> Gui {
    Gui::new(Options {
        demo: true,
        ..Default::default()
    })
    .0
}

#[test]
fn member_list_and_user_card_in_the_demo() {
    let mut gui = demo();
    // Our own server in the demo: its members on the right.
    let _ = gui.update(Message::OpenChannel("2001".into()));
    let list = gui.store.member_lists.get("200").expect("member list");
    let ids: Vec<&str> = list
        .known()
        .filter_map(|r| match r {
            crate::discord::member_list::Row::Member { user_id, .. } => Some(user_id.as_str()),
            _ => None,
        })
        .collect();
    assert!(ids.contains(&"901") && ids.contains(&"902") && ids.contains(&"900"));
    let _ = gui.view();
    // Hidden and shown again from the channel header (saved).
    let _ = gui.update(Message::ToggleMemberList);
    assert!(!gui.settings.member_list);
    let _ = gui.view();
    let _ = gui.update(Message::ToggleMemberList);
    assert!(gui.settings.member_list);

    // The user card: volume only for us, kept by ID.
    let open = || Message::OpenProfile {
        user_id: "902".into(),
        guild_id: Some("200".into()),
    };
    let _ = gui.update(open());
    let _ = gui.view();
    let _ = gui.update(Message::UserVolume {
        user_id: "902".into(),
        percent: 150.0,
    });
    assert_eq!(gui.settings.audio.user_gain("902"), 1.5);
    let _ = gui.update(Message::ToggleUserMute("902".into()));
    assert_eq!(gui.settings.audio.user_gain("902"), 0.0);
    let _ = gui.view();
    let _ = gui.update(Message::ToggleUserMute("902".into()));
    assert_eq!(gui.settings.audio.user_gain("902"), 1.5);
    // Mention: into the message being written, the card closes.
    gui.composer = "hi".into();
    let _ = gui.update(Message::MentionUser("902".into()));
    assert_eq!(gui.composer, "hi <@902> ");
    assert!(gui.profile.is_none());

    // Moderation in our server: kick and ban wait for a confirmation.
    let _ = gui.update(open());
    assert!(gui.store.moderation("200", "902").kick);
    let _ = gui.update(Message::Moderate(ModAction::Kick));
    assert_eq!(gui.profile.as_ref().unwrap().pending, Some(ModAction::Kick));
    let _ = gui.view();
    let _ = gui.update(Message::CancelModeration);
    assert!(gui.profile.as_ref().unwrap().pending.is_none());
    let _ = gui.update(Message::Moderate(ModAction::Timeout(Some(600))));
    let _ = gui.update(Message::ConfirmModeration);
    assert!(gui.profile.as_ref().unwrap().pending.is_none());
    // The demo answers at once, nothing is sent.
    let _ = gui.update(Message::Moderated {
        user_id: "902".into(),
        guild_id: "200".into(),
        action: ModAction::Timeout(Some(600)),
        result: Ok(()),
    });
    assert!(matches!(gui.profile.as_ref().unwrap().outcome, Some(Ok(_))));
    assert!(gui
        .store
        .member("200", "902")
        .and_then(|m| m.communication_disabled_until.as_deref())
        .is_some_and(|u| crate::permissions::timed_out(Some(u))));
    let _ = gui.view();

    // Not where we have no permission, never on ourselves or the owner.
    assert!(!gui.store.moderation("100", "902").any());
    assert!(!gui.store.moderation("200", "900").any());
    assert!(!gui.store.moderation("100", "901").any());
    let _ = gui.update(Message::CloseProfile);
    let _ = gui.update(Message::OpenProfile {
        user_id: "902".into(),
        guild_id: Some("100".into()),
    });
    let _ = gui.update(Message::Moderate(ModAction::Ban));
    assert!(
        gui.profile.as_ref().unwrap().pending.is_none(),
        "not allowed: nothing"
    );
    let _ = gui.view();
}

#[test]
fn reply_in_the_demo() {
    let mut gui = demo();
    let _ = gui.update(Message::StartReply("60008".into()));
    let r = gui.replying.as_ref().expect("replying");
    assert_eq!(r.author, "Giulia");
    assert!(r.ping);
    let _ = gui.view();
    let _ = gui.update(Message::TogglePingReply);
    let _ = gui.update(Message::ComposerChanged("Thanks!".into()));
    let _ = gui.update(Message::Send);
    assert!(gui.replying.is_none());
    let last = gui.store.messages["1001"].last().unwrap();
    assert_eq!(last.content, "Thanks!");
    assert_eq!(
        last.referenced_message.as_ref().map(|r| r.id.as_str()),
        Some("60008")
    );
    // Esc cancels a reply; another channel drops it.
    let _ = gui.update(Message::StartReply("60008".into()));
    let _ = gui.update(Message::ComposerEscape);
    assert!(gui.replying.is_none());
    let _ = gui.update(Message::StartReply("60008".into()));
    let _ = gui.update(Message::OpenChannel("1003".into()));
    assert!(gui.replying.is_none());
}

#[test]
fn no_reply_where_we_cannot_write() {
    let mut gui = demo();
    let _ = gui.update(Message::OpenChannel("1004".into()));
    let id = gui
        .store
        .messages
        .get("1004")
        .and_then(|l| l.first())
        .map(|m| m.id.clone());
    if let Some(id) = id {
        let _ = gui.update(Message::StartReply(id));
    }
    assert!(gui.replying.is_none());
}

#[test]
fn edit_and_delete_our_messages_in_the_demo() {
    let mut gui = demo();
    let _ = gui.update(Message::ComposerChanged("draft".into()));
    // Someone else's message: nothing happens.
    let _ = gui.update(Message::StartEdit("60008".into()));
    assert!(gui.editing.is_none());
    let _ = gui.update(Message::AskDelete("60008".into()));
    assert!(gui.confirm_delete.is_none());

    let _ = gui.update(Message::StartEdit("60007".into()));
    assert_eq!(gui.composer, "Next step is rendering images");
    let _ = gui.view();
    let _ = gui.update(Message::ComposerChanged("Next step: images".into()));
    let _ = gui.update(Message::Send);
    assert!(gui.editing.is_none());
    // The draft comes back after the edit.
    assert_eq!(gui.composer, "draft");
    let m = gui.store.find_message("1001", "60007").unwrap();
    assert_eq!(m.content, "Next step: images");
    assert!(m.edited_timestamp.is_some());

    // Esc leaves the edit without changes.
    let _ = gui.update(Message::StartEdit("60007".into()));
    let _ = gui.update(Message::ComposerChanged("nope".into()));
    let _ = gui.update(Message::ComposerEscape);
    assert_eq!(gui.composer, "draft");
    assert_eq!(
        gui.store.find_message("1001", "60007").unwrap().content,
        "Next step: images"
    );

    // Delete only after the confirmation.
    let _ = gui.update(Message::AskDelete("60007".into()));
    let _ = gui.view();
    let _ = gui.update(Message::CancelDelete);
    assert!(gui.store.find_message("1001", "60007").is_some());
    let _ = gui.update(Message::AskDelete("60007".into()));
    let _ = gui.update(Message::ConfirmDelete);
    assert!(gui.store.find_message("1001", "60007").is_none());
}

#[test]
fn reactions_in_the_demo() {
    let mut gui = demo();
    let crab = Emoji {
        id: None,
        name: Some("🦀".into()),
        animated: false,
    };
    let mine = |gui: &Gui, e: &Emoji| {
        gui.store
            .find_message("1001", "60008")
            .and_then(|m| m.reactions.iter().find(|r| r.emoji.same(e)))
            .map(|r| (r.count, r.me))
    };
    let _ = gui.update(Message::React {
        message_id: "60008".into(),
        emoji: crab.clone(),
        add: false,
    });
    assert_eq!(mine(&gui, &crab), Some((1, false)));
    // The same click twice changes nothing more.
    let _ = gui.update(Message::React {
        message_id: "60008".into(),
        emoji: crab.clone(),
        add: false,
    });
    assert_eq!(mine(&gui, &crab), Some((1, false)));

    // From the picker: a reaction, not text in the field.
    let _ = gui.update(Message::OpenReactionPicker("60008".into()));
    assert!(gui.picker.is_some());
    let _ = gui.update(Message::PickEmoji("🎉"));
    assert!(gui.composer.is_empty());
    assert!(gui.picker.is_none());
    let party = Emoji {
        id: None,
        name: Some("🎉".into()),
        animated: false,
    };
    assert_eq!(mine(&gui, &party), Some((1, true)));
    // A server emoji too.
    let _ = gui.update(Message::OpenReactionPicker("60008".into()));
    let _ = gui.update(Message::PickServerEmoji("cargo".into()));
    let cargo = Emoji {
        id: Some("123456790".into()),
        name: Some("cargo".into()),
        animated: false,
    };
    assert_eq!(mine(&gui, &cargo), Some((1, true)));
    // The composer's emoji button writes again.
    let _ = gui.update(Message::TogglePicker(picker::Tab::Emoji));
    let _ = gui.update(Message::PickEmoji("🦀"));
    assert_eq!(gui.composer, "🦀");
}

#[test]
fn dropped_files_are_limited_and_stay_in_their_channel() {
    let dir = std::env::temp_dir().join(format!("rico-drop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let small = dir.join("note.txt");
    std::fs::write(&small, b"hello").unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let f = rt.block_on(compose::read_file(small.clone(), 1024)).unwrap();
    assert_eq!(f.name, "note.txt");
    assert_eq!(&f.bytes[..], b"hello");
    // Bigger than the room left, a folder, a missing file: refused.
    assert!(rt.block_on(compose::read_file(small.clone(), 4)).is_err());
    assert!(rt.block_on(compose::read_file(dir.clone(), 1024)).is_err());
    assert!(rt
        .block_on(compose::read_file(dir.join("missing"), 1024))
        .is_err());

    let mut gui = demo();
    let _ = gui.update(Message::FileRead(Ok(f.clone())));
    assert_eq!(gui.pending_files.len(), 1);
    let _ = gui.view();
    let _ = gui.update(Message::Send);
    assert!(gui.pending_files.is_empty());
    let last = gui.store.messages["1001"].last().unwrap();
    assert_eq!(last.attachments[0].filename, "note.txt");
    assert_eq!(last.attachments[0].size, 5);

    // At most 10 files.
    for _ in 0..rest::MAX_UPLOAD_FILES {
        let _ = gui.update(Message::FileRead(Ok(f.clone())));
    }
    assert_eq!(gui.pending_files.len(), rest::MAX_UPLOAD_FILES);
    let _ = gui.update(Message::FileDropped(small.clone()));
    assert_eq!(gui.reading_files, 0);
    // Another channel: the files are dropped, never sent there.
    let _ = gui.update(Message::OpenChannel("1003".into()));
    assert!(gui.pending_files.is_empty());
    // Where we cannot write, no file is read.
    let _ = gui.update(Message::OpenChannel("1004".into()));
    let _ = gui.update(Message::FileDropped(small));
    assert_eq!(gui.reading_files, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_member_list_style_draws() {
    use crate::settings::MemberStyle;
    let mut gui = demo();
    // A server channel with its member list.
    let _ = gui.update(Message::OpenChannel("1001".into()));
    assert!(gui.store.member_lists.contains_key("100"));
    for style in MemberStyle::ALL {
        let _ = gui.update(Message::Setting(settings::Change::MemberStyle(style)));
        assert_eq!(gui.settings.member_style, style);
        let _ = gui.view();
    }
    // Compact: the search keeps at most 64 characters.
    let _ = gui.update(Message::MemberFilter("giu".into()));
    let _ = gui.view();
    let _ = gui.update(Message::MemberFilter("x".repeat(500)));
    assert_eq!(gui.member_filter.chars().count(), 64);
    // By activity: offline folded and unfolded.
    let _ = gui.update(Message::Setting(settings::Change::MemberStyle(
        MemberStyle::Activity,
    )));
    let _ = gui.update(Message::ToggleOfflineMembers);
    assert!(gui.offline_open);
    let _ = gui.view();
    let _ = gui.update(Message::ToggleOfflineMembers);
    assert!(!gui.offline_open);
    let _ = gui.update(Message::Setting(settings::Change::MemberStyle(
        MemberStyle::Classic,
    )));
}

#[test]
fn clicking_mentions_opens_the_person_or_the_role() {
    let mut gui = demo();
    // A role mention in #general: the role card with its members.
    let link = mention_link(Some(&Target::Role("7".into())), Some("100"));
    assert!(matches!(&link, Some(Message::OpenRole { role_id, .. }) if role_id == "7"));
    let _ = gui.update(link.unwrap());
    let v = gui.role_view.as_ref().expect("role card");
    assert_eq!(
        v.members.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        ["901"]
    );
    // Giulia is online in the demo's member list.
    assert_eq!(v.online.len(), 1);
    assert!(!v.show_all);
    let _ = gui.view();
    // "See all" opens the whole list.
    let _ = gui.update(Message::RoleShowAll);
    assert!(gui.role_view.as_ref().is_some_and(|v| v.show_all));
    let _ = gui.view();
    // A name in the card opens the profile and closes the card.
    let _ = gui.update(Message::OpenProfile {
        user_id: "901".into(),
        guild_id: Some("100".into()),
    });
    assert!(gui.role_view.is_none());
    assert!(gui.profile.is_some());
    let _ = gui.update(Message::CloseProfile);

    // A user mention opens that person's profile with the server part.
    let link = mention_link(Some(&Target::User("902".into())), Some("100"));
    assert!(matches!(
        &link,
        Some(Message::OpenProfile { user_id, guild_id: Some(g) }) if user_id == "902" && g == "100"
    ));
    // No role card outside servers, nor for servers we are not in.
    assert!(mention_link(Some(&Target::Role("7".into())), None).is_none());
    let _ = gui.update(Message::OpenRole {
        guild_id: "999".into(),
        role_id: "7".into(),
    });
    assert!(gui.role_view.is_none());
    let _ = gui.update(Message::OpenRole {
        guild_id: "100".into(),
        role_id: "../x".into(),
    });
    assert!(gui.role_view.is_none());
    // Esc (CloseRole) closes it.
    let _ = gui.update(Message::OpenRole {
        guild_id: "100".into(),
        role_id: "6".into(),
    });
    let _ = gui.update(Message::CloseRole);
    assert!(gui.role_view.is_none());
}

#[test]
fn update_bar_and_settings() {
    use crate::update::{tests as up, Progress};
    let mut gui = demo();
    // Nothing to show by default; never checked in the demo.
    assert!(gui.update.banner().is_none());
    assert!(!gui.updates_on());
    let _ = gui.update(Message::UpdateCheck);
    assert!(matches!(gui.update.phase, updates::Phase::Idle));
    // Without the owner's keys the settings say so.
    if !crate::update::configured() {
        assert!(gui.update.status_line(true).contains("not"));
    }
    // The switch, on by default.
    assert!(Settings::default().check_updates);
    let _ = gui.update(Message::Setting(settings::Change::CheckUpdates(false)));
    assert!(!gui.settings.check_updates);
    let _ = gui.update(Message::Setting(settings::Change::CheckUpdates(true)));

    // A download in progress, then ready: the bar follows it.
    let key = up::test_key(3);
    let m = up::manifest_json("9.9.9", "stable", "2026-11-18T10:00:00Z");
    let v = crate::update::verify(&m, &up::sign(&key, &m), &[key.verifying_key().to_bytes()]).unwrap();
    gui.update.pending = Some(Box::new(v));
    gui.update.phase = updates::Phase::Downloading {
        version: "9.9.9".into(),
        done: 0,
        total: 1234,
    };
    gui.update_progress(Progress::Bytes {
        done: 617,
        total: 1234,
    });
    assert!(matches!(
        gui.update.phase,
        updates::Phase::Downloading { done: 617, .. }
    ));
    assert!(gui.update.banner().is_some());
    let _ = gui.view();
    gui.update_progress(Progress::Ready(std::path::PathBuf::from("rico-setup-9.9.9.exe")));
    assert!(matches!(gui.update.phase, updates::Phase::Ready { .. }));
    assert!(gui.update.status_line(true).contains("9.9.9") || !crate::update::configured());
    let _ = gui.view();
    // Closed with ×: hidden, but a known flaw is always shown.
    let _ = gui.update(Message::DismissUpdate);
    assert!(gui.update.banner().is_none());
    gui.update.insecure = true;
    assert!(gui.update.banner().is_some());
    // "Restart now" leaves the ready state (the installer is started
    // off the interface thread; here it fails: not Windows or no file).
    let _ = gui.update(Message::RestartToUpdate);
    assert!(matches!(gui.update.phase, updates::Phase::Installing));
    let _ = gui.update(Message::InstallerStarted(Err("test".into())));
    assert!(matches!(gui.update.phase, updates::Phase::Idle));
    // A failed download goes back to idle.
    gui.update_progress(Progress::Failed("network".into()));
    assert_eq!(gui.update.last, updates::LastCheck::CannotCheck);
    // After an update the installer starts Rico with --updated.
    let mut gui = demo();
    gui.update.updated_to = Some("0.18.0".into());
    assert!(gui.update.banner().is_some());
    let _ = gui.update(Message::DismissUpdate);
    assert!(gui.update.banner().is_none());
}

#[test]
fn voice_marks_in_the_demo() {
    use crate::store::VoiceMark as M;
    let mut gui = demo();
    let _ = gui.update(Message::SelectGuild(Some(0)));
    let marks = |gui: &Gui, id: &str| {
        gui.store
            .voice_status
            .get(id)
            .copied()
            .unwrap_or_default()
            .marks()
    };
    assert_eq!(marks(&gui, "902"), [M::SelfMute]);
    assert_eq!(marks(&gui, "903"), [M::ServerMute]);
    assert_eq!(marks(&gui, "904"), [M::SelfDeaf]);
    assert_eq!(marks(&gui, "905"), [M::SelfMute, M::ServerDeaf]);
    // Muted for us too: drawn in the channel list and the member list.
    let _ = gui.update(Message::ToggleUserMute("901".into()));
    let _ = gui.view();
    assert!(voice::marks(crate::store::VoiceFlags::default(), true, 13.0).is_some());
    assert!(voice::marks(crate::store::VoiceFlags::default(), false, 13.0).is_none());
}

#[test]
fn friends_view_in_the_demo() {
    use crate::discord::models::RelationKind;
    use crate::store::FriendsTab;
    use friends::{FriendsMsg, Op};
    let mut gui = demo();
    let _ = gui.update(Message::OpenFriends);
    assert!(gui.friends.open);
    assert_eq!(gui.guild, None);
    assert_eq!(gui.store.current, None);
    let _ = gui.view();
    // Giulia online, Marco idle, Sara offline.
    assert_eq!(gui.store.friends_in(FriendsTab::Online), ["901", "902"]);
    assert_eq!(gui.store.friends_in(FriendsTab::All), ["901", "902", "903"]);
    assert_eq!(gui.store.incoming_requests(), 1);
    for tab in [
        FriendsTab::All,
        FriendsTab::Pending,
        FriendsTab::Blocked,
        FriendsTab::Add,
    ] {
        let _ = gui.update(Message::Friends(FriendsMsg::Tab(tab)));
        let _ = gui.view();
    }
    // Accept Luca's request; a second click does nothing more.
    let _ = gui.update(Message::Friends(FriendsMsg::Do(Op::Accept("904".into()))));
    assert_eq!(gui.store.relation("904"), Some(RelationKind::Friend));
    let _ = gui.update(Message::Friends(FriendsMsg::Do(Op::Accept("904".into()))));
    assert_eq!(gui.store.relation("904"), Some(RelationKind::Friend));
    // Only an incoming request can be accepted.
    let _ = gui.update(Message::Friends(FriendsMsg::Do(Op::Accept("905".into()))));
    assert_eq!(gui.store.relation("905"), Some(RelationKind::Outgoing));
    // Removing a friend asks first; the confirmation removes.
    let _ = gui.update(Message::Friends(FriendsMsg::AskRemove(Some("903".into()))));
    let _ = gui.view();
    assert_eq!(gui.friends.confirm_remove.as_deref(), Some("903"));
    let _ = gui.update(Message::Friends(FriendsMsg::Do(Op::Remove("903".into()))));
    assert_eq!(gui.store.relation("903"), None);
    assert_eq!(gui.friends.confirm_remove, None);
    // Unknown people: nothing happens.
    let _ = gui.update(Message::Friends(FriendsMsg::Do(Op::Remove("77".into()))));
    // The demo sends no friend request, and says so.
    let _ = gui.update(Message::Friends(FriendsMsg::Input("Not A Username!".into())));
    let _ = gui.update(Message::Friends(FriendsMsg::SendRequest));
    assert!(gui.friends.add_result.as_ref().is_some_and(|(_, err)| *err));
    let _ = gui.update(Message::Friends(FriendsMsg::Input("new.friend".into())));
    let _ = gui.update(Message::Friends(FriendsMsg::SendRequest));
    assert!(gui.friends.add_result.as_ref().is_some_and(|(_, err)| !*err));
    assert!(!gui.friends.sending);
    // Opening a conversation leaves the view; a new friend from Discord shows up.
    let _ = gui.update(Message::MessageUser("901".into()));
    assert!(!gui.friends.open);
    assert_eq!(gui.store.current.as_deref(), Some("5001"));
    let _ = gui.update(Message::Gateway(GatewayEvent::RelationshipRemove("901".into())));
    assert_eq!(gui.store.relation("901"), None);
}

#[test]
fn right_click_menu_on_people() {
    use context::UserAction as A;
    let mut gui = demo();
    let open = |gui: &mut Gui, user: &str, guild: Option<&str>| {
        let _ = gui.update(Message::OpenUserMenu {
            user_id: user.into(),
            guild_id: guild.map(str::to_string),
            at: iced::Point::new(10.0, 10.0),
        });
        gui.user_menu
            .as_ref()
            .map(|m| m.actions.clone())
            .unwrap_or_default()
    };

    // Giulia in #general: profile, mention, message, mute for us, copy;
    // no moderation in a server where we have no such permission.
    let actions = open(&mut gui, "901", Some("100"));
    for a in [
        A::Profile,
        A::Mention,
        A::Message,
        A::LocalMute(true),
        A::CopyName,
        A::CopyId,
    ] {
        assert!(actions.contains(&a), "{a:?}");
    }
    assert!(!actions.iter().any(|a| matches!(a, A::Kick | A::Ban | A::Timeout)));
    let _ = gui.view();
    // "Message": our conversation with her opens.
    let _ = gui.update(Message::UserAction(A::Message));
    assert!(gui.user_menu.is_none());
    assert_eq!(gui.store.current.as_deref(), Some("5001"));

    // Ourselves: no message to ourselves, no mute.
    let actions = open(&mut gui, "900", Some("100"));
    assert!(!actions.contains(&A::Message) && !actions.iter().any(|a| matches!(a, A::LocalMute(_))));
    let _ = gui.update(Message::CloseUserMenu);
    assert!(gui.user_menu.is_none());

    // Our own server: moderation of Marco, kick asks for a confirmation
    // in his card.
    let _ = gui.update(Message::OpenChannel("2001".into()));
    let actions = open(&mut gui, "902", Some("200"));
    for a in [A::Timeout, A::Kick, A::Ban] {
        assert!(actions.contains(&a), "{a:?}");
    }
    // Kick: a small confirmation window, no user card.
    let _ = gui.update(Message::UserAction(A::Kick));
    assert!(gui.profile.is_none());
    let p = gui.mod_popup.as_ref().expect("confirmation");
    assert_eq!((p.user_id.as_str(), p.guild_id.as_str()), ("902", "200"));
    assert_eq!(p.step, context::ModStep::Confirm(ModAction::Kick));
    let _ = gui.view();
    let _ = gui.update(Message::CloseModPopup);
    assert!(gui.mod_popup.is_none());
    // Timeout: first the length, then the confirmation.
    let _ = open(&mut gui, "902", Some("200"));
    let _ = gui.update(Message::UserAction(A::Timeout));
    assert_eq!(
        gui.mod_popup.as_ref().map(|p| p.step.clone()),
        Some(context::ModStep::Timeout)
    );
    let _ = gui.view();
    let _ = gui.update(Message::PickModTimeout(300));
    assert_eq!(
        gui.mod_popup.as_ref().map(|p| p.step.clone()),
        Some(context::ModStep::Confirm(ModAction::Timeout(Some(300))))
    );
    let _ = gui.update(Message::ConfirmModPopup);
    assert!(gui.mod_popup.is_none());
    // The answer (in the demo, at once) goes to the status bar.
    let _ = gui.update(Message::Moderated {
        user_id: "902".into(),
        guild_id: "200".into(),
        action: ModAction::Timeout(Some(300)),
        result: Ok(()),
    });
    assert!(gui
        .status
        .as_ref()
        .is_some_and(|(s, error)| !error && !s.is_empty()));
    // A length can be picked only on the lengths step.
    let _ = open(&mut gui, "902", Some("200"));
    let _ = gui.update(Message::UserAction(A::Ban));
    let _ = gui.update(Message::PickModTimeout(60));
    assert_eq!(
        gui.mod_popup.as_ref().map(|p| p.step.clone()),
        Some(context::ModStep::Confirm(ModAction::Ban))
    );
    let _ = gui.update(Message::CloseModPopup);

    // Local mute from the menu, and back.
    let _ = open(&mut gui, "902", Some("200"));
    let _ = gui.update(Message::UserAction(A::LocalMute(true)));
    assert!(gui.settings.audio.muted_users.contains("902"));
    assert_eq!(
        open(&mut gui, "902", Some("200"))
            .iter()
            .find(|a| matches!(a, A::LocalMute(_))),
        Some(&A::LocalMute(false))
    );
    let _ = gui.update(Message::UserAction(A::LocalMute(false)));
    assert!(!gui.settings.audio.muted_users.contains("902"));

    // An action not in the menu does nothing; bad IDs and unknown servers.
    let _ = open(&mut gui, "900", Some("200"));
    let _ = gui.update(Message::UserAction(A::Ban));
    assert!(gui.profile.is_none());
    assert!(open(&mut gui, "../x", None).is_empty());
    let _ = gui.update(Message::CloseUserMenu);
    let actions = open(&mut gui, "902", Some("999"));
    assert!(gui.user_menu.as_ref().is_some_and(|m| m.guild_id.is_none()));
    assert!(!actions.iter().any(|a| matches!(a, A::Kick | A::Ban | A::Timeout)));
    // One menu at a time.
    let _ = gui.update(Message::OpenMessageMenu("60008".into(), iced::Point::ORIGIN));
    assert!(gui.user_menu.is_none());
}

#[test]
fn server_menu_in_the_demo() {
    use context::ServerAction as A;
    let mut gui = demo();
    assert_eq!(gui.store.guild_mentions("100"), 2);
    let _ = gui.update(Message::OpenServerMenu {
        guild_id: "100".into(),
        at: iced::Point::new(10.0, 10.0),
    });
    let actions = gui.server_menu.as_ref().map(|m| m.actions.clone()).unwrap();
    assert_eq!(actions[0], A::MarkRead);
    assert!(actions.contains(&A::Mute(true)) && actions.contains(&A::CopyId));
    let _ = gui.view();
    let _ = gui.update(Message::ServerAction(A::MarkRead));
    assert!(gui.server_menu.is_none());
    assert_eq!(gui.store.guild_mentions("100"), 0);
    assert!(!gui.store.guild_has_unread("100"));
    // Muted for us, then unmuted (only locally in the demo).
    let _ = gui.update(Message::OpenServerMenu {
        guild_id: "100".into(),
        at: iced::Point::ORIGIN,
    });
    let _ = gui.update(Message::ServerAction(A::Mute(true)));
    assert!(gui.store.guild_muted("100"));
    let _ = gui.update(Message::OpenServerMenu {
        guild_id: "100".into(),
        at: iced::Point::ORIGIN,
    });
    assert!(gui
        .server_menu
        .as_ref()
        .unwrap()
        .actions
        .contains(&A::Mute(false)));
    // An action the menu did not offer does nothing.
    let _ = gui.update(Message::ServerAction(A::MarkRead));
    assert!(gui.store.guild_muted("100"));
    let _ = gui.update(Message::CloseServerMenu);
    // An unknown server opens no menu.
    let _ = gui.update(Message::OpenServerMenu {
        guild_id: "999".into(),
        at: iced::Point::ORIGIN,
    });
    assert!(gui.server_menu.is_none());
}

#[test]
fn leaving_asks_first() {
    let mut gui = demo();
    // A server we do not own (the demo's Homelab).
    let _ = gui.update(Message::OpenServerMenu {
        guild_id: "200".into(),
        at: iced::Point::ORIGIN,
    });
    let has_leave = gui
        .server_menu
        .as_ref()
        .is_some_and(|m| m.actions.contains(&context::ServerAction::Leave));
    if has_leave {
        let _ = gui.update(Message::ServerAction(context::ServerAction::Leave));
        assert!(gui.server_menu.as_ref().is_some_and(|m| m.confirm_leave));
        let _ = gui.view();
        let _ = gui.update(Message::ConfirmLeaveServer);
        assert!(gui.server_menu.is_none());
        assert_eq!(gui.store.guilds.len(), 3, "nothing is sent in the demo");
    }
}

#[test]
fn demo_opens_on_general() {
    let gui = demo();
    assert_eq!(gui.store.current.as_deref(), Some("1001"));
    // The demo image is generated locally, no download.
    assert!(matches!(
        gui.previews.get(crate::demo::DEMO_IMAGE_URL),
        Some(Preview::Ready(_))
    ));
    assert_eq!(gui.downloads, 0);
    // The mentions waiting in the demo (#help 2, #storage 1, a group 3).
    assert_eq!(gui.title(), "(6) #general · Rust Community — Rico");
    let _ = gui.view();
}

#[test]
fn stream_preview_from_the_live_badge() {
    let mut gui = demo();
    // Giulia is streaming in the demo's voice channel.
    assert_eq!(gui.store.stream_key("901").as_deref(), Some("guild:100:1021:901"));
    assert!(gui.store.stream_key("902").is_none());
    let _ = gui.view();
    let s = stream::StreamRef {
        guild_id: "100".into(),
        channel_id: "1021".into(),
        user_id: "901".into(),
    };
    let _ = gui.update(Message::OpenStreamPreview(s.clone()));
    let url = gui
        .stream_view
        .as_ref()
        .and_then(|v| v.image_url())
        .unwrap()
        .to_string();
    assert!(matches!(gui.small_images.get(&url), Some(Preview::Ready(_))));
    let _ = gui.view();
    // Too soon to ask again.
    assert!(!gui.stream_view.as_ref().unwrap().can_refresh());
    // A late answer for another stream does not touch the card.
    let other = stream::StreamRef {
        user_id: "902".into(),
        ..s
    };
    let _ = gui.update(Message::StreamPreviewLoaded {
        stream: other,
        result: Ok(None),
    });
    assert!(gui.stream_view.as_ref().unwrap().image_url().is_some());
    // The stream ends while the card is open: no crash, the card says so.
    gui.store.streaming.remove("901");
    let _ = gui.view();
    let _ = gui.update(Message::CloseStreamPreview);
    assert!(gui.stream_view.is_none());
}

#[test]
fn server_folders_in_the_demo() {
    use crate::store::RailEntry;
    let mut gui = demo();
    // Rust Community alone, then the "Hobby" folder with the other two.
    let rail = gui.store.rail();
    assert!(matches!(rail[0], RailEntry::Guild(0)));
    let RailEntry::Folder { id, guilds, name, .. } = &rail[1] else {
        panic!("no folder")
    };
    assert_eq!((name.as_deref(), guilds.as_slice()), (Some("Hobby"), &[1, 2][..]));
    let id = *id;
    let _ = gui.view();
    let _ = gui.update(Message::ToggleFolder(id));
    assert!(gui.open_folders.contains(&id));
    let _ = gui.view();
    // Editor: Rust Community into the folder, rename, save (demo: only here).
    let _ = gui.update(Message::OpenFolderEditor);
    let _ = gui.view();
    let _ = gui.update(Message::FolderEdit(folders::Edit::Move(
        "100".into(),
        folders::Target::Folder(id, "Hobby".into()),
    )));
    let _ = gui.update(Message::FolderEdit(folders::Edit::Rename(id, "Tutto".into())));
    let _ = gui.view();
    let _ = gui.update(Message::SaveFolders);
    assert!(gui.folder_editor.is_none());
    let rail = gui.store.rail();
    assert_eq!(rail.len(), 1);
    assert!(
        matches!(&rail[0], RailEntry::Folder { name, guilds, .. } if name.as_deref() == Some("Tutto") && guilds.len() == 3)
    );
    // A settings update from another device (partial, folders only) wins.
    let mut gf = crate::discord::folders::GuildFolders::default();
    gf.normalize(&gui.store.guild_ids());
    let proto = crate::discord::folders::encode_folders_update(&gf);
    let ev: gateway::SettingsProtoUpdate = serde_json::from_value(serde_json::json!({
        "settings": {"type": 1, "proto": proto}, "partial": true
    }))
    .unwrap();
    let _ = gui.update(Message::Gateway(GatewayEvent::SettingsProto(ev)));
    assert_eq!(gui.store.rail().len(), 3);
}

#[test]
fn settings_window_in_the_demo() {
    let mut gui = demo();
    let _ = gui.update(Message::OpenSettings);
    assert!(gui.settings_panel.is_some());
    for section in [
        settings::Section::Account,
        settings::Section::Appearance,
        settings::Section::Accessibility,
        settings::Section::Voice,
        settings::Section::TextImages,
        settings::Section::Language,
        settings::Section::Shortcuts,
        settings::Section::About,
    ] {
        let _ = gui.update(Message::SettingsSection(section));
        let _ = gui.view();
    }
    // Compact messages, 12-hour clock, no images, no reactions: still drawn.
    for change in [
        settings::Change::Compact(true),
        settings::Change::Clock24h(false),
        settings::Change::ShowImages(false),
        settings::Change::ShowReactions(false),
        settings::Change::Autoplay(false),
        settings::Change::OutputVolume(150.0),
    ] {
        let _ = gui.update(Message::Setting(change));
    }
    assert!(gui.settings.compact && !gui.settings.show_images);
    assert_eq!(gui.settings.audio.output_volume, 1.5);
    let _ = gui.update(Message::CloseSettings);
    assert!(gui.settings_panel.is_none());
    let _ = gui.view();
    // Each theme can be drawn; back to the dark one for the other tests.
    for theme in ThemeChoice::ALL {
        let _ = gui.update(Message::Setting(settings::Change::Theme(theme)));
        let _ = gui.view();
    }
    let _ = gui.update(Message::Setting(settings::Change::Theme(ThemeChoice::Light)));
    assert_eq!(style::pal::bg(), style::colors_of(ThemeChoice::Light, true).bg);
    let _ = gui.update(Message::Setting(settings::Change::Theme(ThemeChoice::Dark)));
    anim::set_autoplay(true);
}

#[test]
fn message_menu_in_the_demo() {
    let mut gui = demo();
    // Someone else's message: no edit or delete.
    let _ = gui.update(Message::OpenMessageMenu(
        "60008".into(),
        iced::Point::new(10.0, 20.0),
    ));
    let menu = gui.message_menu.clone().expect("menu");
    assert!(menu.reply && menu.copy_text && !menu.edit && !menu.delete);
    let mine = "60007".to_string();
    let _ = gui.update(Message::OpenMessageMenu(
        mine.clone(),
        iced::Point::new(10.0, 20.0),
    ));
    let menu = gui.message_menu.clone().expect("menu");
    assert!(menu.reply && menu.edit && menu.delete);
    let _ = gui.view();
    let _ = gui.update(Message::MessageAction(context::Action::Reply));
    assert!(gui.message_menu.is_none());
    assert!(gui.replying.is_some());
    // Delete asks for the confirmation first, as from the bar.
    let _ = gui.update(Message::OpenMessageMenu(mine.clone(), iced::Point::ORIGIN));
    let _ = gui.update(Message::MessageAction(context::Action::Delete));
    assert_eq!(gui.confirm_delete.as_deref(), Some(mine.as_str()));
    // Unknown messages open nothing; Esc-like close.
    let _ = gui.update(Message::OpenMessageMenu("nope".into(), iced::Point::ORIGIN));
    assert!(gui.message_menu.is_none());
    let _ = gui.update(Message::OpenMessageMenu(mine, iced::Point::ORIGIN));
    let _ = gui.update(Message::CloseMessageMenu);
    assert!(gui.message_menu.is_none());
}

#[test]
fn status_menu_in_the_demo() {
    use crate::discord::folders::Presence;
    let mut gui = demo();
    let _ = gui.update(Message::OpenStatusMenu);
    assert!(gui.status_menu.is_some());
    let _ = gui.view();
    let _ = gui.update(Message::SetPresence(Presence::DoNotDisturb));
    let status_of = |g: &Gui| {
        g.store
            .my_status
            .as_ref()
            .map(crate::discord::folders::StatusSettings::presence)
    };
    assert_eq!(status_of(&gui), Some(Presence::DoNotDisturb));
    let _ = gui.update(Message::OpenStatusMenu);
    let _ = gui.update(Message::StatusEdit(presence::Edit::Text(" At work ".into())));
    let _ = gui.update(Message::SaveCustomStatus);
    assert!(gui.status_menu.is_none());
    let custom = |g: &Gui| g.store.my_status.as_ref().and_then(|s| s.custom.clone());
    assert_eq!(custom(&gui).map(|c| c.text), Some("At work".to_string()));
    // The status stays when the custom status changes.
    assert_eq!(status_of(&gui), Some(Presence::DoNotDisturb));
    let _ = gui.view();
    let _ = gui.update(Message::OpenStatusMenu);
    let _ = gui.update(Message::ClearCustomStatus);
    assert_eq!(custom(&gui), None);
    let _ = gui.update(Message::OpenStatusMenu);
    let _ = gui.update(Message::CloseStatusMenu);
    assert!(gui.status_menu.is_none());
}

#[test]
fn watching_a_stream_in_the_demo() {
    let mut gui = demo();
    let s = stream::StreamRef {
        guild_id: "100".into(),
        channel_id: "1021".into(),
        user_id: "901".into(),
    };
    let _ = gui.update(Message::OpenStreamPreview(s));
    let _ = gui.update(Message::WatchStream);
    // Watching needs the voice channel: joined, microphone off.
    assert_eq!(gui.voice.as_ref().map(|v| v.channel_id.as_str()), Some("1021"));
    assert!(gui.stream_view.is_none());
    let w = gui.watch.as_ref().expect("watching");
    assert_eq!(w.key, "guild:100:1021:901");
    // The demo's moving picture arrives.
    let mut waited = 0;
    while !w.frames.has_picture() {
        std::thread::sleep(std::time::Duration::from_millis(10));
        waited += 1;
        assert!(waited < 300, "no picture");
    }
    assert_eq!(w.frames.size(), Some((640, 360)));
    let _ = gui.view();
    // Volume: slider, arrow steps (up to 200%), mute keeps the level.
    let volume = |gui: &Gui| {
        gui.watch
            .as_ref()
            .map(|w| (w.volume, w.effective_volume()))
            .unwrap()
    };
    let _ = gui.update(Message::StreamVolume(150.0));
    assert_eq!(volume(&gui), (1.5, 1.5));
    let _ = gui.update(Message::StreamVolumeStep(1));
    assert!((volume(&gui).0 - 1.6).abs() < 1e-6);
    for _ in 0..10 {
        let _ = gui.update(Message::StreamVolumeStep(1));
    }
    assert_eq!(volume(&gui), (2.0, 2.0));
    let _ = gui.update(Message::ToggleStreamSound);
    assert_eq!(volume(&gui), (2.0, 0.0));
    let _ = gui.view();
    let _ = gui.update(Message::StreamVolumeStep(-1));
    assert!((volume(&gui).1 - 1.9).abs() < 1e-6, "a change unmutes");
    // Discord ends the stream: the view says so.
    let _ = gui.update(Message::Gateway(GatewayEvent::StreamDelete(
        serde_json::from_str(r#"{"stream_key": "guild:100:1021:901", "reason": "stream_full"}"#).unwrap(),
    )));
    assert!(matches!(
        gui.watch.as_ref().unwrap().status,
        watch::Status::Ended(_)
    ));
    let _ = gui.view();
    // Stopping keeps us in the voice channel.
    let _ = gui.update(Message::StopWatching);
    assert!(gui.watch.is_none());
    assert!(gui.voice.is_some());
    // Leaving the channel also stops watching.
    let _ = gui.update(Message::OpenStreamPreview(stream::StreamRef {
        guild_id: "100".into(),
        channel_id: "1021".into(),
        user_id: "901".into(),
    }));
    let _ = gui.update(Message::WatchStream);
    // The next stream starts at the last volume chosen.
    assert!((gui.watch.as_ref().unwrap().volume - 1.9).abs() < 1e-6);
    let _ = gui.update(Message::LeaveVoice);
    assert!(gui.watch.is_none());
}

#[test]
fn hovering_someone_streaming_shows_the_small_preview() {
    let mut gui = demo();
    let s = stream::StreamRef {
        guild_id: "100".into(),
        channel_id: "1021".into(),
        user_id: "901".into(),
    };
    let _ = gui.update(Message::HoverStream(s.clone()));
    let p = gui.hover_previews.get("guild:100:1021:901").expect("asked");
    let url = p.image_url().unwrap().to_string();
    assert!(matches!(gui.small_images.get(&url), Some(Preview::Ready(_))));
    let asked = p.asked;
    let _ = gui.view();
    // Hovering again soon after does not ask again.
    let _ = gui.update(Message::HoverStream(s.clone()));
    assert_eq!(gui.hover_previews.get("guild:100:1021:901").unwrap().asked, asked);
    // Someone not streaming, or a made-up stream: nothing is asked.
    let _ = gui.update(Message::HoverStream(stream::StreamRef {
        user_id: "902".into(),
        ..s.clone()
    }));
    let _ = gui.update(Message::HoverStream(stream::StreamRef {
        channel_id: "1023".into(),
        ..s.clone()
    }));
    assert_eq!(gui.hover_previews.len(), 1);
    // An answer from Discord updates the small preview.
    let _ = gui.update(Message::StreamPreviewLoaded {
        stream: s.clone(),
        result: Ok(None),
    });
    assert!(matches!(
        gui.hover_previews.get("guild:100:1021:901").unwrap().state,
        stream::PreviewState::Unavailable
    ));
    // A click on them in the list watches the stream directly.
    let _ = gui.update(Message::WatchStreamOf(s));
    assert!(gui.watch.is_some());
    let _ = gui.view();
}

#[test]
fn stream_window_people_and_full_screen() {
    let mut gui = demo();
    let s = stream::StreamRef {
        guild_id: "100".into(),
        channel_id: "1021".into(),
        user_id: "901".into(),
    };
    let _ = gui.update(Message::OpenStreamPreview(s.clone()));
    let _ = gui.update(Message::WatchStream);
    assert!(gui.watch.is_some());
    // The people of the channel, with their microphone and sound.
    assert!(gui
        .store
        .voice_status
        .get("902")
        .is_some_and(|f| f.muted() && !f.deafened()));
    assert!(gui.store.voice_status.get("901").is_some_and(|f| !f.muted()));
    assert!(gui.watch_members);
    let _ = gui.view();
    let _ = gui.update(Message::ToggleWatchMembers);
    assert!(!gui.watch_members);
    let _ = gui.view();
    let _ = gui.update(Message::ToggleWatchMembers);
    // The stream already watched: nothing restarts.
    let attempt = gui.watch.as_ref().unwrap().attempt;
    let _ = gui.update(Message::WatchStreamOf(s));
    assert_eq!(gui.watch.as_ref().unwrap().attempt, attempt);
    // Full screen: button or F11, then Esc leaves it before stopping.
    let _ = gui.update(Message::ToggleFullscreen);
    assert!(gui.fullscreen);
    let _ = gui.view();
    let _ = gui.update(Message::WatchControls(true));
    assert!(gui.watch.as_ref().unwrap().controls_shown);
    let _ = gui.view();
    let _ = gui.update(Message::WatchEscape);
    assert!(!gui.fullscreen);
    assert!(gui.watch.is_some());
    // Two quick clicks on the video: full screen; one click: nothing.
    let _ = gui.update(Message::VideoPressed);
    assert!(!gui.fullscreen);
    let _ = gui.update(Message::VideoPressed);
    assert!(gui.fullscreen);
    // Whatever closes the stream also leaves full screen.
    let _ = gui.update(Message::LeaveVoice);
    assert!(gui.watch.is_none());
    assert!(!gui.fullscreen);
    // Without a stream, F11 does nothing.
    let _ = gui.update(Message::ToggleFullscreen);
    assert!(!gui.fullscreen);
}

#[test]
fn permissions_in_the_interface() {
    let mut gui = demo();
    // Read-only channel: nothing is sent, not even a GIF.
    let _ = gui.update(Message::OpenChannel("1004".into()));
    let before = gui.store.messages.get("1004").map_or(0, Vec::len);
    let _ = gui.update(Message::ComposerChanged("hello".into()));
    let _ = gui.update(Message::Send);
    let _ = gui.send_text("https://tenor.com/view/x-1".into());
    assert_eq!(gui.store.messages.get("1004").map_or(0, Vec::len), before);
    let _ = gui.view();

    // Voice channel we cannot join: no join, an explanation.
    let _ = gui.update(Message::JoinVoice("1023".into()));
    assert!(gui.voice.is_none());
    assert!(gui.status.as_ref().is_some_and(|(_, error)| *error));
    let _ = gui.view();

    // The open channel becomes hidden: the client moves to a visible one.
    let _ = gui.update(Message::OpenChannel("1001".into()));
    let mut ch = gui.store.guilds[0]
        .channels
        .iter()
        .find(|c| c.id == "1001")
        .unwrap()
        .clone();
    ch.permission_overwrites =
        serde_json::from_str(r#"[{"id": "100", "type": 0, "allow": "0", "deny": "1024"}]"#).unwrap();
    let _ = gui.update(Message::Gateway(GatewayEvent::ChannelUpdate(Box::new(ch))));
    assert_ne!(gui.store.current.as_deref(), Some("1001"));
    assert!(gui.store.current.is_some());
    let _ = gui.view();
}

#[test]
fn navigation_and_sending() {
    let mut gui = demo();
    let _ = gui.update(Message::SelectGuild(Some(1)));
    assert_eq!(gui.store.current.as_deref(), Some("2001"));
    let _ = gui.view();

    let _ = gui.update(Message::SelectGuild(None));
    assert_eq!(gui.store.current.as_deref(), Some("5001"));
    let _ = gui.view();

    let _ = gui.update(Message::OpenChannel("1001".into()));
    let _ = gui.update(Message::ComposerChanged("  hello from the GUI  ".into()));
    let _ = gui.update(Message::Send);
    assert_eq!(
        gui.store.messages["1001"].last().unwrap().content,
        "hello from the GUI"
    );
    assert!(gui.composer.is_empty());

    let id = gui.store.messages["1001"][0].id.clone();
    let _ = gui.update(Message::HoverStart(id.clone()));
    assert_eq!(gui.hovered.as_deref(), Some(id.as_str()));
    let _ = gui.view();
    let _ = gui.update(Message::HoverEnd(id));
    assert!(gui.hovered.is_none());
}

#[test]
fn login_screen() {
    let mut gui = Gui::new(Options {
        demo_login: true,
        ..Default::default()
    })
    .0;
    assert_eq!(gui.screen, Screen::Login);
    let _ = gui.view();
    let _ = gui.update(Message::LoginTab(LoginTab::Token));
    let _ = gui.update(Message::TokenInput("Bot abc".into()));
    let _ = gui.update(Message::SubmitToken);
    assert!(gui.login.error.is_some());
    let _ = gui.view();
}

#[test]
fn previews_downloaded_a_few_at_a_time() {
    let mut gui = demo();
    let attachments: Vec<serde_json::Value> = (0..10)
        .map(|i| {
            serde_json::json!({
                "id": i.to_string(), "filename": format!("foto{i}.png"), "size": 1000,
                "url": format!("https://cdn.discordapp.com/attachments/1/{i}/foto.png"),
                "proxy_url": format!("https://media.discordapp.net/attachments/1/{i}/foto.png"),
                "content_type": "image/png", "width": 64, "height": 64
            })
        })
        .collect();
    let mut msg = crate::demo::local_message("1001", None, "", gui.store.messages.get("1001"));
    msg.attachments = serde_json::from_value(serde_json::Value::Array(attachments)).unwrap();
    gui.store.insert_message(msg);
    let _ = gui.request_previews();
    assert_eq!(gui.downloads, MAX_DOWNLOADS);
    let loading = gui
        .previews
        .values()
        .filter(|p| matches!(p, Preview::Loading))
        .count();
    assert_eq!(loading, MAX_DOWNLOADS);
    let _ = gui.view();

    // They start in attachment order: the seventh waits.
    let url = "https://media.discordapp.net/attachments/1/0/foto.png".to_string();
    let seventh = "https://media.discordapp.net/attachments/1/6/foto.png".to_string();
    assert!(matches!(gui.previews.get(&url), Some(Preview::Loading)));
    assert!(!gui.previews.contains_key(&seventh));

    // A failed download starts another one and leaves the file name.
    let _ = gui.update(Message::PreviewLoaded {
        url: url.clone(),
        result: Err("test".into()),
    });
    assert!(matches!(gui.previews.get(&url), Some(Preview::Failed)));
    assert!(matches!(gui.previews.get(&seventh), Some(Preview::Loading)));
    assert_eq!(gui.downloads, MAX_DOWNLOADS);
    let _ = gui.view();
}

#[test]
fn previews_of_old_messages_too() {
    let mut gui = demo();
    // An image followed by many text-only messages (as after loading the
    // older history by scrolling up).
    let mut msg = crate::demo::local_message("1001", None, "", gui.store.messages.get("1001"));
    msg.attachments = serde_json::from_value(serde_json::json!([{
        "id": "1", "filename": "vecchia.jpg", "size": 1000,
        "url": "https://cdn.discordapp.com/attachments/1/1/vecchia.jpg",
        "proxy_url": "https://media.discordapp.net/attachments/1/1/vecchia.jpg",
        "content_type": "image/jpeg", "width": 64, "height": 64
    }]))
    .unwrap();
    gui.store.insert_message(msg);
    for i in 0..120 {
        let m =
            crate::demo::local_message("1001", None, &format!("text {i}"), gui.store.messages.get("1001"));
        gui.store.insert_message(m);
    }
    let _ = gui.request_previews();
    let url = "https://media.discordapp.net/attachments/1/1/vecchia.jpg";
    assert!(matches!(gui.previews.get(url), Some(Preview::Loading)));
}

#[test]
fn full_screen_view() {
    let mut gui = demo();
    let att = gui.store.messages["1001"]
        .iter()
        .flat_map(|m| m.attachments.iter())
        .find(|a| a.url == crate::demo::DEMO_IMAGE_URL)
        .unwrap()
        .clone();
    let _ = gui.update(Message::OpenViewer(Box::new(att)));
    assert!(matches!(
        gui.viewer.as_ref().map(|v| &v.image),
        Some(Preview::Ready(_))
    ));
    let _ = gui.view();
    // Nothing is downloaded in the demo.
    let _ = gui.update(Message::SaveAttachment);
    assert!(matches!(
        gui.viewer.as_ref().map(|v| &v.save),
        Some(SaveState::Idle)
    ));
    let _ = gui.update(Message::Noop);
    assert!(gui.viewer.is_some());
    let _ = gui.update(Message::CloseViewer);
    assert!(gui.viewer.is_none());

    // Real attachment: the download of the large version starts.
    let real: Attachment = serde_json::from_value(serde_json::json!({
        "id": "9", "filename": "foto.jpg", "size": 2048,
        "url": "https://cdn.discordapp.com/attachments/1/9/foto.jpg",
        "proxy_url": "https://media.discordapp.net/attachments/1/9/foto.jpg",
        "content_type": "image/jpeg", "width": 4000, "height": 3000
    }))
    .unwrap();
    let _ = gui.update(Message::OpenViewer(Box::new(real)));
    let v = gui.viewer.as_ref().unwrap();
    assert!(matches!(v.image, Preview::Loading));
    let url = v.url.clone().unwrap();
    assert!(url.ends_with("width=2560&height=1920"), "{url}");
    let _ = gui.view();
    let _ = gui.update(Message::ViewerLoaded {
        url,
        result: Err("test".into()),
    });
    assert!(matches!(gui.viewer.as_ref().unwrap().image, Preview::Failed));
    let _ = gui.update(Message::Saved(Err("disk full".into())));
    assert!(matches!(gui.viewer.as_ref().unwrap().save, SaveState::Failed(_)));
    let _ = gui.view();
}

#[test]
fn gifs_decoded_at_the_screen_size() {
    let (w, h) = media::display_size(1000, 400);
    // At scale 1.19 a bit more than the logical size, never the double.
    let (dw, dh) = decode_size(1000, 400, 1.19);
    assert_eq!((dw, dh), ((w * 1.19).ceil() as u32, (h * 1.19).ceil() as u32));
    assert!(dw < (w * 2.0) as u32);
    // Never more than the original.
    assert_eq!(decode_size(100, 40, 3.0), (100, 40));
    // The window tells its scale; out-of-range values are clamped.
    let mut gui = demo();
    let _ = gui.update(Message::ScaleFactor(1.25));
    assert_eq!(gui.scale, 1.25);
    let _ = gui.update(Message::ScaleFactor(0.0));
    assert_eq!(gui.scale, 1.0);
}

#[test]
fn link_gifs_are_animated() {
    let gui = demo();
    // The demo GIF is generated locally and animated.
    match gui.previews.get(crate::demo::DEMO_GIF_URL) {
        Some(Preview::Animated(a)) => assert!(a.frames_in_memory() > 1),
        _ => panic!("demo GIF not animated"),
    }
    let _ = gui.view();
}

#[test]
fn real_gif_falling_back_to_the_static_image() {
    let mut gui = demo();
    let mut msg = crate::demo::local_message(
        "1001",
        None,
        "https://tenor.com/view/gatto-1",
        gui.store.messages.get("1001"),
    );
    msg.embeds = serde_json::from_value(serde_json::json!([{
        "type": "gifv", "url": "https://tenor.com/view/gatto-1", "provider": {"name": "Tenor"},
        "thumbnail": {"url": "https://media.tenor.com/abcAAAAe/gatto.png",
            "proxy_url": "https://images-ext-1.discordapp.net/external/s/https/media.tenor.com/abcAAAAe/gatto.png",
            "width": 498, "height": 280},
        "video": {"url": "https://media.tenor.com/abcAAAPo/gatto.mp4",
            "proxy_url": "https://images-ext-1.discordapp.net/external/v/https/media.tenor.com/abcAAAPo/gatto.mp4",
            "width": 498, "height": 280}
    }]))
    .unwrap();
    gui.store.insert_message(msg);
    let _ = gui.request_previews();
    // The clip from Discord's proxy first (never Tenor itself).
    let gif =
        "https://images-ext-1.discordapp.net/external/v/https/media.tenor.com/abcAAAPo/gatto.mp4".to_string();
    let still =
        "https://images-ext-1.discordapp.net/external/s/https/media.tenor.com/abcAAAAe/gatto.png".to_string();
    assert!(matches!(gui.previews.get(&gif), Some(Preview::Loading)));
    assert!(!gui.previews.contains_key(&still));
    let _ = gui.view();
    assert!(!gui
        .previews
        .keys()
        .any(|u| u.starts_with("https://media.tenor.com")));
    // If the clip does not arrive, Discord's static image is used.
    let _ = gui.update(Message::PreviewLoaded {
        url: gif,
        result: Err("404".into()),
    });
    assert!(matches!(gui.previews.get(&still), Some(Preview::Loading)));
    let _ = gui.view();
}

#[test]
fn emoji_picker() {
    let mut gui = demo();
    gui.composer = "hi ".into();
    let _ = gui.update(Message::TogglePicker(picker::Tab::Emoji));
    assert!(gui.picker.is_some());
    let _ = gui.view();
    let _ = gui.update(Message::EmojiQuery("crab".into()));
    let _ = gui.view();
    let _ = gui.update(Message::PickEmoji("🦀"));
    assert_eq!(gui.composer, "hi 🦀");
    assert!(gui.picker.is_none());
    assert_eq!(gui.recent_emojis, vec!["🦀"]);
    // Reopening starts from the recently used; the same button closes it.
    let _ = gui.update(Message::TogglePicker(picker::Tab::Emoji));
    assert_eq!(gui.picker.as_ref().unwrap().section, picker::Section::Recent);
    let _ = gui.view();
    let _ = gui.update(Message::TogglePicker(picker::Tab::Emoji));
    assert!(gui.picker.is_none());
}

#[test]
fn gif_picker() {
    let mut gui = demo();
    let before = gui.store.messages["1001"].len();
    // Previews are off until turned on (they come from Klipy): titles only.
    assert!(!gui.settings.gif_picker_previews);
    let _ = gui.update(Message::TogglePicker(picker::Tab::Gif));
    let p = gui.picker.as_ref().unwrap();
    assert!(!p.shown_gifs().is_empty());
    let _ = gui.view();
    // The line under the titles opens the settings on "Text & images".
    let _ = gui.update(Message::OpenGifSettings);
    assert!(gui.picker.is_none());
    assert_eq!(
        gui.settings_panel.as_ref().map(|p| p.section),
        Some(settings::Section::TextImages)
    );
    let _ = gui.view();
    let _ = gui.update(Message::Setting(settings::Change::GifPickerPreviews(true)));
    assert!(gui.settings.gif_picker_previews);
    let _ = gui.update(Message::CloseSettings);
    let _ = gui.update(Message::TogglePicker(picker::Tab::Gif));
    assert!(matches!(
        gui.previews.get(crate::demo::DEMO_GIF_URL),
        Some(Preview::Animated(_))
    ));
    let _ = gui.view();
    // An old search does not overwrite the new one.
    let _ = gui.update(Message::GifQuery("ferr".into()));
    let _ = gui.update(Message::GifQuery("ferris".into()));
    let _ = gui.update(Message::GifSearch(1));
    assert!(!gui.picker.as_ref().unwrap().searching);
    let _ = gui.update(Message::GifSearch(2));
    let url = gui.picker.as_ref().unwrap().shown_gifs()[0].url.clone();
    // Click on a GIF: the link is sent and the GIF shows up in the chat.
    gui.composer = "draft".into();
    let _ = gui.update(Message::PickGif(url.clone()));
    assert!(gui.picker.is_none());
    assert_eq!(gui.composer, "draft");
    let list = &gui.store.messages["1001"];
    assert_eq!(list.len(), before + 1);
    assert_eq!(list.last().unwrap().content, url);
    assert_eq!(list.last().unwrap().embeds.len(), 1);
    let _ = gui.view();
}

#[test]
fn server_emojis_only_in_their_server() {
    let mut gui = demo();
    // The emojis in the channel's messages already have their image.
    let ferris = media::emoji_url("123456789", false);
    assert!(matches!(gui.small_images.get(&ferris), Some(Preview::Ready(_))));
    // In the picker: those of the server, without the animated ones (no
    // Nitro) and without those reserved to a role.
    let _ = gui.update(Message::TogglePicker(picker::Tab::Emoji));
    assert_eq!(gui.picker.as_ref().unwrap().section, picker::Section::Server);
    let usable: Vec<_> = gui
        .store
        .usable_emojis("1001")
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert_eq!(usable, ["ferris", "cargo", "clippy"]);
    assert!(gui
        .small_images
        .contains_key(&media::emoji_url("123456790", false)));
    let _ = gui.view();
    let _ = gui.update(Message::PickServerEmoji("cargo".into()));
    assert_eq!(gui.composer, ":cargo:");
    gui.composer.push_str(" hi :ferris: :ferris_party:");
    let _ = gui.update(Message::Send);
    let last = gui.store.messages["1001"].last().unwrap();
    assert_eq!(
        last.content,
        "<:cargo:123456790> hi <:ferris:123456789> :ferris_party:"
    );
    let _ = gui.view();

    // In a direct message server emojis are not used.
    let _ = gui.update(Message::OpenChannel("5001".into()));
    assert!(gui.store.usable_emojis("5001").is_empty());
    gui.composer = ":ferris:".into();
    let _ = gui.update(Message::Send);
    assert_eq!(gui.store.messages["5001"].last().unwrap().content, ":ferris:");
    let _ = gui.update(Message::TogglePicker(picker::Tab::Emoji));
    assert_ne!(gui.picker.as_ref().unwrap().section, picker::Section::Server);
    let _ = gui.view();
}

#[test]
fn email_and_password_login_answers() {
    let mut gui = Gui::blank();
    let _ = gui.update(Message::LoginTab(LoginTab::Password));
    let _ = gui.view();
    // Nothing written: nothing sent.
    let _ = gui.update(Message::SubmitPassword);
    assert!(!gui.login.busy && gui.login.error.is_some());
    let _ = gui.update(Message::EmailInput("me@example.com".into()));
    let _ = gui.update(Message::PasswordInput("secret".into()));
    assert!(gui.login.error.is_none());

    // Two-factor code asked: the password is dropped, the ticket kept.
    let two_factor = || Ok(Answer::TwoFactor(TwoFactor::for_tests(true)));
    let _ = gui.update(Message::PasswordAnswer(two_factor()));
    assert!(gui.login.password.is_empty() && gui.login.two_factor.is_some());
    let _ = gui.view();
    let _ = gui.update(Message::CodeInput("123 456 789 012 345".into()));
    assert_eq!(gui.login.code.chars().count(), 12);
    let _ = gui.update(Message::PasswordAnswer(Ok(Answer::Refused("no".into()))));
    assert!(gui.login.two_factor.is_some() && gui.login.code.is_empty());

    // New location: back to the form, with the email kept, to sign in again.
    let _ = gui.update(Message::PasswordAnswer(Ok(Answer::NewLocation)));
    assert!(gui.login.two_factor.is_none());
    assert_eq!(gui.login.email, "me@example.com");
    assert_eq!(
        gui.login.notice.as_deref(),
        Some(password_login::new_location_notice())
    );
    let _ = gui.view();

    // Only SMS or a security key: not handled, said so.
    let _ = gui.update(Message::PasswordInput("secret".into()));
    let _ = gui.update(Message::PasswordAnswer(Ok(Answer::TwoFactor(
        TwoFactor::for_tests(false),
    ))));
    assert!(gui.login.two_factor.is_none() && gui.login.error.is_some());
    assert!(gui.login.password.is_empty());

    // No answer from Discord: the fields stay, to try again.
    let _ = gui.update(Message::PasswordInput("secret".into()));
    let _ = gui.update(Message::PasswordAnswer(Err("offline".into())));
    assert_eq!(gui.login.password, "secret");
    let _ = gui.update(Message::PasswordAnswer(Ok(Answer::Captcha)));
    assert_eq!(gui.login.error.as_deref(), Some(password_login::captcha_notice()));
    assert_eq!(gui.screen, Screen::Login);
}

#[test]
fn our_stream_in_the_demo() {
    let mut gui = demo();
    // Not in a call: nothing opens.
    let _ = gui.update(Message::OpenGoLive);
    assert!(gui.golive_picker.is_none());
    let _ = gui.update(Message::JoinVoice("1021".into()));
    let _ = gui.update(Message::Voice(VoiceEvent::Connected {
        mode: "aead_aes256_gcm_rtpsize",
    }));
    let _ = gui.view();
    let _ = gui.update(Message::OpenGoLive);
    assert!(gui.golive_picker.is_some());
    let _ = gui.view();
    // The list arrives (here, as the task would give it): the test picture first.
    let _ = gui.update(Message::GoLiveSources(golive::sources(true)));
    let picker = gui.golive_picker.as_ref().unwrap();
    assert_eq!(
        picker.source().unwrap().kind,
        crate::capture::SourceKind::TestPattern
    );
    let _ = gui.update(Message::GoLiveQuality(crate::video::encoder::Quality::Sd30));
    let _ = gui.update(Message::GoLiveSound(false));
    assert!(!gui.settings.golive_sound);
    let _ = gui.view();
    let _ = gui.update(Message::StartGoLive);
    assert!(gui.golive_picker.is_none());
    let g = gui.golive.as_ref().expect("stream not started");
    assert!(g.demo && g.running.sound.is_none());
    assert_eq!(g.quality, crate::video::encoder::Quality::Sd30);
    // Our LIVE badge next to us, and "Live" in the call panel.
    let me = gui.store.me_id().unwrap().to_string();
    assert!(gui.store.streaming.contains(&me));
    let _ = gui.update(Message::GoLiveVoice(VoiceEvent::Connected {
        mode: "aead_aes256_gcm_rtpsize",
    }));
    assert_eq!(gui.golive.as_ref().unwrap().status, golive::Status::Connected);
    let _ = gui.view();
    // Stop: the capture is dropped, the badge goes away.
    let _ = gui.update(Message::StopGoLive);
    assert!(gui.golive.is_none());
    assert!(!gui.store.streaming.contains(&me));
    // Leaving the call also ends a stream.
    let _ = gui.update(Message::OpenGoLive);
    let _ = gui.update(Message::GoLiveSources(golive::sources(true)));
    let _ = gui.update(Message::StartGoLive);
    assert!(gui.golive.is_some());
    let _ = gui.update(Message::LeaveVoice);
    assert!(gui.golive.is_none());
}

#[test]
fn voice_chat_in_the_demo() {
    let mut gui = demo();
    // Giulia and Marco are already in the demo voice channel.
    assert_eq!(gui.store.voice_members("1021").len(), 2);
    let _ = gui.update(Message::JoinVoice("1021".into()));
    let v = gui.voice.as_ref().expect("voice not started");
    assert!(v.demo);
    assert_eq!(v.status, voice::Status::Connecting);
    let _ = gui.view();
    // Connection events (in the demo they come from the fake server).
    let _ = gui.update(Message::Voice(VoiceEvent::Connected {
        mode: "aead_aes256_gcm_rtpsize",
    }));
    let _ = gui.update(Message::Voice(VoiceEvent::Speaking {
        user_id: "901".into(),
        speaking: true,
    }));
    assert_eq!(gui.store.voice_members("1021").len(), 3);
    assert!(gui.voice.as_ref().unwrap().speaking.contains("901"));
    let _ = gui.view();
    let _ = gui.update(Message::LeaveVoice);
    assert!(gui.voice.is_none());
    assert_eq!(gui.store.voice_members("1021").len(), 2);

    // A final error stays visible until leaving.
    let _ = gui.update(Message::JoinVoice("1021".into()));
    let _ = gui.update(Message::Voice(VoiceEvent::Closed(Some("test".into()))));
    assert_eq!(
        gui.voice.as_ref().unwrap().status,
        voice::Status::Failed("test".into())
    );
    let _ = gui.view();
    // In the same channel again: try again.
    let _ = gui.update(Message::JoinVoice("1021".into()));
    assert_eq!(gui.voice.as_ref().unwrap().status, voice::Status::Connecting);
}

#[test]
fn switching_voice_channel_survives_the_late_leave_update() {
    let mut gui = demo();
    gui.demo = false;
    let (link, mut commands) = crate::discord::gateway::commands();
    gui.gateway_link = Some(link);
    gui.store.upsert_channel(
        serde_json::from_value(serde_json::json!({
            "id": "1022", "type": 2, "name": "Second room", "guild_id": "100", "parent_id": "1020"
        }))
        .unwrap(),
    );
    let me = gui.store.me_id().unwrap().to_string();
    let state = |channel: Option<&str>| {
        let v: VoiceState = serde_json::from_value(serde_json::json!({
            "guild_id": "100", "channel_id": channel, "user_id": me, "session_id": "s1"
        }))
        .unwrap();
        Message::Gateway(GatewayEvent::VoiceState(v))
    };

    let _ = gui.update(Message::JoinVoice("1021".into()));
    let _ = gui.update(state(Some("1021")));
    assert!(gui.voice.as_ref().unwrap().confirmed);
    // Switch: only "join 1022" goes to Discord, no "leave".
    while commands.try_recv().is_ok() {}
    let _ = gui.update(Message::JoinVoice("1022".into()));
    let sent: Vec<_> = std::iter::from_fn(|| commands.try_recv().ok()).collect();
    assert_eq!(sent.len(), 1);
    assert!(format!("{:?}", sent[0]).contains("1022"));
    // A late "left" for the old channel does not close the new connection.
    let _ = gui.update(state(None));
    assert_eq!(gui.voice.as_ref().unwrap().channel_id, "1022");
    let _ = gui.update(state(Some("1022")));
    assert_eq!(gui.voice.as_ref().unwrap().session_id.as_deref(), Some("s1"));
    // Once confirmed, leaving for real closes it.
    let _ = gui.update(state(None));
    assert!(gui.voice.is_none());

    // Asking again for the channel Discord already has us in: the
    // session id is reused, since Discord does not send it again.
    let _ = gui.update(state(Some("1021")));
    let _ = gui.update(Message::JoinVoice("1021".into()));
    assert_eq!(gui.voice.as_ref().unwrap().session_id.as_deref(), Some("s1"));
}

#[test]
fn avatars_icons_and_profiles_in_the_demo() {
    let mut gui = demo();
    // Server icon and avatars are made locally in the demo, no downloads.
    let icon = media::guild_icon_url("100", gui.store.guild("100").and_then(|g| g.icon()), ICON_PX).unwrap();
    assert!(matches!(gui.small_images.get(&icon), Some(Preview::Ready(_))));
    let giulia = gui.store.avatar_url(Some("100"), "901", AVATAR_PX).unwrap();
    assert!(matches!(gui.small_images.get(&giulia), Some(Preview::Ready(_))));
    assert_eq!(gui.small_downloads, 0);
    // Server nickname instead of the account name.
    let marco = gui.store.users["902"].clone();
    assert_eq!(gui.store.name_in(Some("100"), &marco), "Marco (ops)");

    // Click on Giulia in the server: the server profile with her roles.
    let _ = gui.update(Message::OpenProfile {
        user_id: "901".into(),
        guild_id: Some("100".into()),
    });
    let p = gui.profile.as_ref().expect("profile not open");
    assert_eq!(p.tab, profile::Tab::Server);
    let profile::Data::Ready(data) = &p.data else {
        panic!("demo profile not ready")
    };
    assert_eq!(data.guild_member.as_ref().unwrap().roles, vec!["5", "7"]);
    let _ = gui.view();
    let _ = gui.update(Message::ProfileTab(profile::Tab::Account));
    let _ = gui.view();
    let _ = gui.update(Message::CloseProfile);
    assert!(gui.profile.is_none());

    // Unknown user: an error in the card, not a crash.
    let _ = gui.update(Message::OpenProfile {
        user_id: "123".into(),
        guild_id: None,
    });
    assert!(matches!(
        gui.profile.as_ref().unwrap().data,
        profile::Data::Failed(_)
    ));
    let _ = gui.view();
    let _ = gui.update(Message::CloseProfile);

    // Direct messages: Giulia's avatar next to her conversation; the
    // group without a picture shows the initials.
    let _ = gui.update(Message::SelectGuild(None));
    let dm = gui.store.dms.iter().find(|c| c.id == "5001").unwrap().clone();
    let (colour, url) = gui.store.dm_picture(&dm, AVATAR_PX);
    assert_eq!(colour, "901");
    assert!(matches!(
        gui.small_images.get(&url.unwrap()),
        Some(Preview::Ready(_))
    ));
    let group = gui.store.dms.iter().find(|c| c.id == "5002").unwrap().clone();
    assert_eq!(
        gui.store.dm_picture(&group, AVATAR_PX),
        ("5002".to_string(), None)
    );
    let _ = gui.view();
}

#[test]
fn microphone_starts_off_and_follows_the_connection() {
    let mut gui = demo();
    let _ = gui.update(Message::JoinVoice("1021".into()));
    let _ = gui.update(Message::Voice(VoiceEvent::Connected {
        mode: "aead_aes256_gcm_rtpsize",
    }));
    let v = gui.voice.as_ref().unwrap();
    assert!(!v.mic_on && !v.deafened);
    assert_eq!(v.encryption, voice_net::Encryption::Pending);
    // Alone in the channel: shown as such, not as a setup that hangs.
    let _ = gui.update(Message::Voice(VoiceEvent::Encryption(
        voice_net::Encryption::Alone,
    )));
    assert_eq!(
        gui.voice.as_ref().unwrap().encryption,
        voice_net::Encryption::Alone
    );
    let _ = gui.view();
    let _ = gui.update(Message::Voice(VoiceEvent::Encryption(
        voice_net::Encryption::Active {
            privacy_code: "123451234512345123451234512345".into(),
        },
    )));
    // The state shown is the one confirmed by the connection.
    let _ = gui.update(Message::Voice(VoiceEvent::Microphone(true)));
    assert!(gui.voice.as_ref().unwrap().mic_on);
    let _ = gui.view();
    let _ = gui.update(Message::ToggleDeafen);
    assert!(gui.voice.as_ref().unwrap().deafened);
    let _ = gui.update(Message::Voice(VoiceEvent::Microphone(false)));
    assert!(!gui.voice.as_ref().unwrap().mic_on);
    let _ = gui.update(Message::Voice(VoiceEvent::Closed(Some("test".into()))));
    assert!(!gui.voice.as_ref().unwrap().mic_on);
}

#[test]
fn preview_cache_is_bounded() {
    let mut gui = demo();
    for i in 0..(MAX_PREVIEWS + 20) {
        gui.remember_preview(format!("u{i}"), Preview::Failed);
    }
    assert_eq!(gui.previews.len(), MAX_PREVIEWS);
    assert!(!gui.previews.contains_key("u0"));
    assert!(gui.previews.contains_key(&format!("u{}", MAX_PREVIEWS + 19)));
}

#[test]
fn readable_sizes() {
    assert_eq!(human_size(245_760), "240 KB");
    assert_eq!(human_size(512), "512 B");
    assert_eq!(human_size(0), "");
}

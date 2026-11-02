use keryx_core::queue::{Level, Queue, Target};

fn queue() -> Queue<&'static str> {
    Queue::default()
}

fn next_item(queue: &mut Queue<&'static str>) -> Option<&'static str> {
    let outgoing = queue.take_next(false)?;
    Some(outgoing.item)
}

#[test]
fn levels_go_in_priority_order() {
    let mut queue = queue();
    queue.push(Level::Content, "text", Target::None);
    queue.push(Level::Typing, "typing", Target::None);
    queue.push(Level::Keys, "commit", Target::None);
    assert_eq!(next_item(&mut queue), Some("commit"));
    assert_eq!(next_item(&mut queue), Some("typing"));
    assert_eq!(next_item(&mut queue), Some("text"));
    assert_eq!(next_item(&mut queue), None);
}

#[test]
fn content_keeps_the_order_it_was_sent_in() {
    let mut queue = queue();
    queue.push(Level::Content, "first", Target::None);
    queue.push(Level::Content, "second", Target::Seq(9));
    assert_eq!(next_item(&mut queue), Some("first"));
    let second = queue.take_next(false).unwrap();
    assert_eq!(second.item, "second");
    assert_eq!(second.pointer_seq, Some(9));
}

#[test]
fn an_action_waits_for_its_targets_sequence_id_and_nothing_overtakes_it() {
    let mut queue = queue();
    let target = queue.push(Level::Content, "target", Target::None);
    queue.push(Level::Content, "reply", Target::Pending(target));
    queue.push(Level::Content, "later", Target::None);

    assert_eq!(next_item(&mut queue), Some("target"));
    assert_eq!(next_item(&mut queue), None);

    queue.push(Level::Keys, "commit", Target::None);
    assert_eq!(next_item(&mut queue), Some("commit"));
    assert_eq!(next_item(&mut queue), None);

    queue.confirm(target, 41);
    let reply = queue.take_next(false).unwrap();
    assert_eq!(reply.item, "reply");
    assert_eq!(reply.pointer_seq, Some(41));
    assert_eq!(next_item(&mut queue), Some("later"));
}

#[test]
fn typing_is_never_waited_on() {
    let mut queue = queue();
    queue.push(Level::Typing, "typing", Target::None);
    assert_eq!(next_item(&mut queue), Some("typing"));
    assert!(queue.in_flight_list().is_empty());
    assert!(queue.is_empty());
}

#[test]
fn after_a_reconnect_unanswered_items_go_again_in_order() {
    let mut queue = queue();
    let first = queue.push(Level::Content, "first", Target::None);
    let second = queue.push(Level::Content, "second", Target::None);
    let third = queue.push(Level::Content, "third", Target::None);
    let commit = queue.push(Level::Keys, "commit", Target::None);
    queue.push(Level::Typing, "stale", Target::None);
    let _ = queue.take_next(false);
    let _ = queue.take_next(false);
    let _ = queue.take_next(false);
    let _ = queue.take_next(false);
    assert_eq!(
        queue.in_flight_list(),
        vec![(commit, &"commit"), (first, &"first"), (second, &"second")]
    );

    // The updates after reconnecting held the first one, so only the others go again.
    queue.confirm(first, 7);
    queue.push(Level::Typing, "stale too", Target::None);
    queue.reconnected();
    assert_eq!(next_item(&mut queue), Some("commit"));
    assert_eq!(next_item(&mut queue), Some("second"));
    assert_eq!(next_item(&mut queue), Some("third"));
    assert_eq!(next_item(&mut queue), None);

    queue.confirm(commit, 8);
    queue.confirm(second, 9);
    queue.confirm(third, 10);
    assert!(queue.is_empty());
}

#[test]
fn giving_up_on_a_message_takes_whatever_waits_on_it() {
    let mut queue = queue();
    let target = queue.push(Level::Content, "refused", Target::None);
    let reply = queue.push(Level::Content, "reply", Target::Pending(target));
    let reaction = queue.push(Level::Content, "reaction", Target::Pending(reply));
    let free = queue.push(Level::Content, "free", Target::None);
    let _ = queue.take_next(false);

    assert_eq!(
        queue.drop_with_dependents(target),
        vec![target, reply, reaction]
    );
    assert_eq!(queue.take_next(false).unwrap().id, free);
    queue.confirm(free, 1);
    assert!(queue.is_empty());
}

#[test]
fn a_lost_commit_can_be_dropped() {
    let mut queue = queue();
    let commit = queue.push(Level::Keys, "commit", Target::None);
    let typing = queue.push(Level::Typing, "typing", Target::None);
    assert_eq!(queue.drop_with_dependents(typing), vec![typing]);
    let _ = queue.take_next(false);
    assert_eq!(queue.drop_with_dependents(commit), vec![commit]);
    assert!(queue.is_empty());
}

#[test]
fn only_typing_goes_while_a_commit_waits() {
    let mut queue = queue();
    queue.push(Level::Content, "text", Target::None);
    queue.push(Level::Keys, "commit", Target::None);
    queue.push(Level::Typing, "typing", Target::None);
    assert_eq!(queue.take_next(true).unwrap().item, "typing");
    assert!(queue.take_next(true).is_none());
    assert_eq!(queue.take_next(false).unwrap().item, "commit");
    assert_eq!(queue.take_next(false).unwrap().item, "text");
}

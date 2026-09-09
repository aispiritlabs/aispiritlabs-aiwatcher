-- A hosted decider's messages are a third kind.
--
-- `workflow_messages.kind` was `command | event` because those were the two a
-- Rust decider produces. A hosted execution's history is the worker's own, and
-- this engine stores it without reading it: the type name, the worker's
-- metadata, and a reference to the content. It is neither an intention this
-- engine may refuse nor a fact it folds, so filing it as either would be a
-- column that disagrees with the row beside it.
--
-- Widening rather than dropping: the constraint still refuses a fourth spelling,
-- which is what it is for. `0001` is not edited — a version is recorded once and
-- skipped forever after, so a rewrite would reach no database that already ran
-- it and would only make two installations at one version disagree about what
-- that version did.
alter table workflow_messages
    drop constraint if exists workflow_messages_kind_check;

alter table workflow_messages
    add constraint workflow_messages_kind_check
    check (kind in ('command', 'event', 'hosted'));

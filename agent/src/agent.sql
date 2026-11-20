-- keryx-agent's SQLite schema, agent.db. Compiled into the binary with include_str! by store.rs, and
-- run the first time it opens a new file.
--
-- Every field names the feature that needs it, in [brackets]. A field with no feature left to name
-- should be dropped.
--
-- Rules for the whole file, the server's:
--   Ids are ULIDs in BLOB(16). A chat's messages are keyed by (chat, seq).
--   Times are unix milliseconds.
--   Enums are integers, with their values listed beside the column.
--
-- Tables of small rows are WITHOUT ROWID. Tables holding large values are not, as the server's.
--
-- This file holds the words' keys inside core's snapshot, the session token, and, in the run log,
-- what chats said: whoever reads it reads every chat the agent is in. See agent/AGENTS.md.


-- ---------------------------------------------------------------------------
-- What core keeps
-- ---------------------------------------------------------------------------

-- [Snapshot] core's two slots, written alternately, so a torn write never loses the only copy.
CREATE TABLE snapshot (
    -- [Snapshot] 0 or 1.
    slot                INTEGER PRIMARY KEY CHECK (slot IN (0, 1)),
    -- [Snapshot] the whole device state, as core handed it over.
    bytes               BLOB NOT NULL,
    -- [Snapshot] when it was written.
    saved_at            INTEGER NOT NULL
);

-- [Login] the session core handed back, for Loaded, so a restart needs no new login.
CREATE TABLE session (
    -- [Login] always 0: one account per process.
    id                  INTEGER PRIMARY KEY CHECK (id = 0),
    -- [Login] the session token. Never logged.
    token               TEXT NOT NULL
) WITHOUT ROWID;

-- [Archive] every message as the server sent it, ciphertext, header and signature, as Android's.
-- A removed message keeps only its chat, seq and server time.
CREATE TABLE message (
    -- [Archive] the chat and the message's place in it.
    chat                BLOB NOT NULL,
    seq                 INTEGER NOT NULL CHECK (seq > 0),
    -- [Archive, Edits] 0 content | 1 edit | 2 delete | 3 commit | 4 device notice | 5 line. NULL once
    -- removed.
    kind                INTEGER,
    -- [Search, Waking] the schema's content kind, 0 text to 15 activity. NULL for what is not content.
    content_kind        INTEGER,
    -- [Restart] which device sent it, so this device's own unanswered sends are matched.
    device              BLOB,
    -- [Edits, Deletes] an edit's or delete's readable target.
    pointer             INTEGER,
    -- [Topics] a group's or channel's topic, NULL for General.
    topic               BLOB,
    -- [Threads] the main-list message whose thread this is in.
    thread_seq          INTEGER,
    -- [Search, Waking] the server's time.
    saved_at            INTEGER NOT NULL,
    -- [Archive] the frame as JSON, exactly as the server sent it.
    frame               TEXT NOT NULL,
    PRIMARY KEY (chat, seq)
);

-- [Search] the scan reads every chat newest first by server time.
CREATE INDEX message_saved ON message (saved_at, chat, seq);

-- [Calls] what happened inside a call, kept beside its message so its replay opens.
CREATE TABLE call_row (
    -- [Calls] the call's chat, its message's seq and its place in it.
    chat                BLOB NOT NULL,
    call_seq            INTEGER NOT NULL,
    n                   INTEGER NOT NULL,
    -- [Calls] 0 join | 1 leave | 2 decline | 3 text | 4 end.
    kind                INTEGER NOT NULL,
    -- [Calls] on an end: 0 missed | 1 answered.
    outcome             INTEGER,
    -- [Calls] the row as JSON, as the server sent it.
    frame               TEXT NOT NULL,
    PRIMARY KEY (chat, call_seq, n)
);

-- [History] a group's or channel's history rows as the server sent them, a delete's copies still
-- ciphertext, as Android's. Chosen by the author while building.
CREATE TABLE chat_event (
    -- [History] the chat and the row's place in its history.
    chat                BLOB NOT NULL,
    n                   INTEGER NOT NULL,
    -- [History] 0 everyone | 1 owners and moderators | 2 one account.
    audience            INTEGER NOT NULL,
    -- [History] the server's time, so a page of messages finds the rows among it.
    server_at           INTEGER NOT NULL,
    -- [History] the row as JSON, as the server sent it.
    frame               TEXT NOT NULL,
    PRIMARY KEY (chat, n)
);


-- ---------------------------------------------------------------------------
-- The two queues
-- ---------------------------------------------------------------------------

-- [Waking, Run log] everything that may wake the loop, kept once taken as the record of what woke it.
CREATE TABLE event (
    -- [Waking] its id, which inbox.defer names.
    id                  BLOB PRIMARY KEY,
    -- [Waking] 0 message | 1 reaction | 2 answer | 3 ack | 4 added | 5 removed | 6 invitation |
    -- 7 join_request | 8 edit | 9 delete | 10 schedule | 11 storage | 12 sent.
    kind                INTEGER NOT NULL CHECK (kind BETWEEN 0 AND 12),
    -- [Grouping] its chat, topic and thread, NULL where it has none.
    chat                BLOB,
    topic               BLOB,
    thread_seq          INTEGER,
    -- [Seen, Context] the message it is about.
    seq                 INTEGER,
    -- [Waking] what the loop is shown, as JSON.
    body                TEXT NOT NULL,
    -- [Waking] when it arrived here.
    received_at         INTEGER NOT NULL,
    -- [Waking] 0 waiting | 1 taken | 2 deferred.
    state               INTEGER NOT NULL DEFAULT 0 CHECK (state IN (0, 1, 2)),
    -- [inbox.defer] a deferred event's earliest time, NULL for the next wake.
    until               INTEGER,
    -- [Run log] the run and step that took it.
    run                 BLOB,
    step                INTEGER
);

-- [Waking] what is waiting, oldest first.
CREATE INDEX event_waiting ON event (state, received_at);

-- [Writes, Restart] everything sent to the server, kept until answered, so a restart sends it again
-- or finds it already stored.
CREATE TABLE outgoing (
    -- [Writes] its id, which a queued write returns.
    id                  BLOB PRIMARY KEY,
    -- [Run log] the run, step and action that made it.
    run                 BLOB,
    step                INTEGER,
    n                   INTEGER,
    -- [Writes] its chat.
    chat                BLOB NOT NULL,
    -- [Writes] core's command, as JSON.
    command             TEXT NOT NULL,
    -- [Restart] when it was handed to core, to match a stored copy by.
    client_at           INTEGER NOT NULL,
    -- [Writes] 0 queued | 1 sent | 2 done | 3 refused.
    state               INTEGER NOT NULL DEFAULT 0 CHECK (state IN (0, 1, 2, 3)),
    -- [Writes] the stored message's seq, once it has one.
    seq                 INTEGER,
    -- [Writes] the server's refusal.
    code                TEXT,
    -- [Writes] when its state last changed.
    at                  INTEGER NOT NULL
);

-- [Restart] what is still to go or to be answered.
CREATE INDEX outgoing_open ON outgoing (state, client_at);


-- ---------------------------------------------------------------------------
-- The run log
-- ---------------------------------------------------------------------------

-- [Run log] one wake of the loop.
CREATE TABLE run (
    id                  BLOB PRIMARY KEY,
    started_at          INTEGER NOT NULL,
    ended_at            INTEGER,
    -- [Run log] 0 running | 1 done | 2 limit | 3 failed.
    state               INTEGER NOT NULL DEFAULT 0 CHECK (state BETWEEN 0 AND 3)
) WITHOUT ROWID;

-- [Run log, Restart] one tool call of a run, saved after every phase so a restart goes on from it.
CREATE TABLE step (
    run                 BLOB NOT NULL,
    n                   INTEGER NOT NULL,
    -- [Restart] 1 input built | 2 LLM call sent | 3 answer | 4 script started | 5 output |
    -- 6 script finished.
    phase               INTEGER NOT NULL CHECK (phase BETWEEN 1 AND 6),
    -- [Run log] what this step added to the input, as JSON.
    context             TEXT NOT NULL,
    -- [Activity] what the step said it is for.
    intention_list      TEXT,
    -- [Restart] the LLM's answer, as JSON.
    answer              TEXT,
    -- [Run log] the script it ran.
    script              TEXT,
    sent_at             INTEGER,
    started_at          INTEGER,
    ended_at            INTEGER,
    -- [Restart] 1 when a restart cut its script short.
    cut                 INTEGER NOT NULL DEFAULT 0 CHECK (cut IN (0, 1)),
    -- [Run log] what the provider counted.
    tokens_in           INTEGER,
    tokens_out          INTEGER,
    PRIMARY KEY (run, n)
);

-- [Run log, Restart] each print and each action of a step's script, inserted as it happens.
CREATE TABLE step_output (
    run                 BLOB NOT NULL,
    step                INTEGER NOT NULL,
    n                   INTEGER NOT NULL,
    -- [Run log] 0 print | 1 action.
    kind                INTEGER NOT NULL CHECK (kind IN (0, 1)),
    -- [Run log] the printed text, or the action as JSON.
    body                TEXT NOT NULL,
    at                  INTEGER NOT NULL,
    PRIMARY KEY (run, step, n)
);


-- ---------------------------------------------------------------------------
-- Schedules and memory
-- ---------------------------------------------------------------------------

-- [Schedules] what the model left for its future self.
CREATE TABLE schedule (
    id                  BLOB PRIMARY KEY,
    -- [Schedules] 0 at | 1 every | 2 cron.
    kind                INTEGER NOT NULL CHECK (kind IN (0, 1, 2)),
    -- [Schedules] the time, the duration or the cron expression, as written.
    spec                TEXT NOT NULL,
    -- [Schedules] a cron's IANA zone.
    tz                  TEXT NOT NULL,
    -- [Schedules] an every's first time.
    start               INTEGER,
    -- [Schedules] what the fire tells the loop.
    note                TEXT NOT NULL,
    -- [Schedules] the chats whose nearby messages come with the fire, 16 bytes each.
    chats               BLOB NOT NULL,
    -- [Schedules] 1 to join a running loop at its next step, 0 to wait until it is idle.
    join_run            INTEGER NOT NULL CHECK (join_run IN (0, 1)),
    -- [Schedules] the next fire.
    next_at             INTEGER NOT NULL,
    fired_at            INTEGER,
    -- [Run log] the run that made it.
    run                 BLOB,
    made_at             INTEGER NOT NULL
) WITHOUT ROWID;

-- [Schedules] the next to fire.
CREATE INDEX schedule_next ON schedule (next_at);

-- [Memory] notes, global or a chat's, searched only.
CREATE TABLE memory (
    id                  BLOB PRIMARY KEY,
    -- [Memory] the chat whose memory it is, NULL for the global one.
    chat                BLOB,
    text                TEXT NOT NULL,
    -- [Memory] where it came from: the run and step, and the chat and message it was about.
    run                 BLOB NOT NULL,
    step                INTEGER NOT NULL,
    source_chat         BLOB,
    seq                 INTEGER,
    made_at             INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL
);

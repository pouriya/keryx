-- keryx's SQLite schema. Compiled into the binary with include_str!, and run the first time a
-- command opens a new database.
--
-- Every field names the feature that needs it, in [brackets]. A field with no feature
-- left to name should be dropped.
--
-- Rules for the whole file:
--   Ids are ULIDs in BLOB(16). A chat's messages are keyed by (conversation_id, seq).
--   Times are unix milliseconds.
--   Enums are integers, with their values listed beside the column.
--   Foreign keys need `PRAGMA foreign_keys = ON` on every connection.
--
-- Tables of small rows are WITHOUT ROWID. Tables holding large blobs are not, because
-- SQLite's single writer blocks every chat while it works: in measurement a rowid table
-- inserted a message 1.5 to 2.4 times faster, with page reads under 0.2 ms either way.
--
-- Nothing here holds a word, a private key or the plaintext of an encrypted message.


-- ---------------------------------------------------------------------------
-- Accounts and logins
-- ---------------------------------------------------------------------------

CREATE TABLE account (
    -- [Everything] who a chat member, device or role belongs to.
    id                  BLOB PRIMARY KEY,
    -- [Registration, Login, Directory] a-z, 0-9 and _, not starting with a digit. The 3 to
    -- 32 length is checked in the code and not here, because `keryx user add` may break it.
    username            TEXT NOT NULL UNIQUE CHECK (
                            length(username) > 0
                            AND username NOT GLOB '*[^a-z0-9_]*'
                            AND username NOT GLOB '[0-9]*'
                        ),
    -- [Directory, Chat list] any language, not unique. The only profile field the server can
    -- read, because the directory searches it. The rest is in account_profile.
    display_name        TEXT,
    -- [Login] Argon2id. NULL until the one-time link from `keryx user add` is used.
    password_hash       TEXT,
    -- [Device stamps, Termination, Password reset] verifies what the words sign. NULL until
    -- the words are set up.
    master_public_key   BLOB,
    -- [Restore, Key wrapping] what epoch keys are wrapped to. NULL until the words are set up.
    account_public_key  BLOB,
    -- [Key wrapping] the master key's signature over account_public_key, so a server cannot
    -- swap in its own key and be sent every chat's keys. NULL until the words are set up.
    account_key_stamp   BLOB,
    -- [Words setup] when setup finished, so the client knows whether to ask for it.
    words_set_at        INTEGER,
    -- [Seen times] a person may turn receipts off. Their times are still stored.
    receipts_enabled    INTEGER NOT NULL DEFAULT 1 CHECK (receipts_enabled IN (0, 1)),
    -- [Admin] when the account was made.
    created_at          INTEGER NOT NULL
) WITHOUT ROWID, STRICT;

-- [Account profile] one encrypted blob per account: image info and its file key, bio, and
-- links as key and value. Readable only by people sharing a chat with this account.
CREATE TABLE account_profile (
    -- [Account profile] whose profile.
    account_id  BLOB PRIMARY KEY REFERENCES account (id) ON DELETE CASCADE,
    -- [Account profile] says which profile key opens it, so a client can tell when it needs
    -- a newer one.
    key_version INTEGER NOT NULL,
    -- [Account profile] the blob, which the server cannot read.
    sealed      BLOB NOT NULL,
    -- [Signatures] by updated_by, so a server cannot forge someone's profile.
    signature   BLOB NOT NULL,
    -- [Signatures] which device changed it.
    updated_by  BLOB NOT NULL REFERENCES device (id),
    -- [Account profile] lets a client skip fetching a blob it already has.
    updated_at  INTEGER NOT NULL
) STRICT;

-- [Account profile] an account's profile key, shared with one chat under that chat's key.
-- This is one row per chat, not one profile per chat, so changing a profile rewrites one blob.
-- The key does not rotate when someone leaves, and channels get no rows here.
CREATE TABLE account_profile_key (
    -- [Account profile] whose profile this opens.
    account_id       BLOB NOT NULL REFERENCES account (id) ON DELETE CASCADE,
    -- [Account profile] which chat's members can open it.
    conversation_id  BLOB NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
    -- [Account profile] matched against account_profile.key_version.
    key_version      INTEGER NOT NULL,
    -- [Account profile] the chat key this was encrypted under. Re-encrypted when a member is
    -- added, because a new member only has the current key.
    epoch            INTEGER NOT NULL,
    -- [Account profile] the profile key, encrypted under that chat key.
    sealed_key       BLOB NOT NULL,
    PRIMARY KEY (account_id, conversation_id)
) WITHOUT ROWID, STRICT;

-- [Account profile] re-encrypting a chat's profile keys when a member is added.
CREATE INDEX account_profile_key_chat ON account_profile_key (conversation_id);

-- [Registration off] the one-time link `keryx user add` prints.
CREATE TABLE account_setup (
    -- [Registration off] only the hash is stored, so a stolen database opens no links.
    token_hash  BLOB PRIMARY KEY,
    -- [Registration off] whose password and words this link sets.
    account_id  BLOB NOT NULL REFERENCES account (id) ON DELETE CASCADE,
    -- [Registration off] an unused link stops working.
    expires_at  INTEGER NOT NULL,
    -- [Registration off] set when the password is chosen, so a later try can say the link was used.
    used_at     INTEGER
) WITHOUT ROWID, STRICT;

CREATE TABLE login_session (
    -- [Session list] what a person names when revoking one.
    id            BLOB PRIMARY KEY,
    -- [Login] only the cookie token's hash is stored, so a stolen database logs nobody in.
    token_hash    BLOB NOT NULL UNIQUE,
    -- [Login] who this session is.
    account_id    BLOB NOT NULL REFERENCES account (id) ON DELETE CASCADE,
    -- [Device limit, Reduced view] NULL means password only: reduced device list, and
    -- messaging refused.
    device_id     BLOB REFERENCES device (id),
    -- [Session list] shown when a person reviews their sessions.
    created_at    INTEGER NOT NULL,
    -- [Session list, Sliding sessions] each use updates it.
    last_used_at  INTEGER NOT NULL,
    -- [Sliding sessions] each use moves it forward by the configured length.
    expires_at    INTEGER NOT NULL
) WITHOUT ROWID, STRICT;

-- [Session list]
CREATE INDEX login_session_account ON login_session (account_id);
-- [Sliding sessions] the sweep for expired ones.
CREATE INDEX login_session_expiry ON login_session (expires_at);

-- [Password reset, Returning device, Device ids] what the server handed out to be used once: a
-- challenge to sign, or a new device's id, so a client can never choose its own.
CREATE TABLE challenge (
    -- [Password reset, Returning device, Device ids] 32 random bytes, or a device's ULID.
    value       BLOB PRIMARY KEY,
    -- [Password reset, Returning device, Device ids] which account it is for.
    account_id  BLOB NOT NULL REFERENCES account (id) ON DELETE CASCADE,
    -- [Password reset, Returning device, Device ids] 0 password reset, 1 returning device,
    -- 2 device id: so one cannot be used as another.
    purpose     INTEGER NOT NULL CHECK (purpose IN (0, 1, 2)),
    -- [Password reset, Returning device, Device ids] five minutes after it was made.
    expires_at  INTEGER NOT NULL
) WITHOUT ROWID, STRICT;

-- [Captcha] used up by one check, right or wrong.
CREATE TABLE captcha (
    -- [Captcha] what the client sends back with its answer.
    id          BLOB PRIMARY KEY,
    -- [Captcha] compared on check, and fixed by `test_answer` in tests.
    answer      TEXT NOT NULL,
    -- [Captcha] `ttl`, and when the sweep may delete it, with `keep_for`.
    expires_at  INTEGER NOT NULL
) WITHOUT ROWID, STRICT;

-- [Captcha] the sweep.
CREATE INDEX captcha_expiry ON captcha (expires_at);


-- ---------------------------------------------------------------------------
-- Devices
-- ---------------------------------------------------------------------------

-- [Device list, Signatures] never deleted: old messages and seen rows name these devices,
-- and old signatures have to stay checkable.
CREATE TABLE device (
    -- [Device list, Messages, Seen] every message and seen row names its device.
    id                    BLOB PRIMARY KEY,
    -- [Device list, Device limit] whose device this is.
    account_id            BLOB NOT NULL REFERENCES account (id),
    -- [Signatures] verifies this device's messages and its logout.
    signing_public_key    BLOB NOT NULL,
    -- [Device stamps] the master key's signature saying this device is the account's.
    -- Members check it before accepting the device into a chat.
    master_stamp          BLOB NOT NULL,
    -- [Device list] 0 desktop, 1 mobile. A claim from the device, which can be faked.
    platform              INTEGER NOT NULL CHECK (platform IN (0, 1)),
    -- [Device list] a claim from the device.
    os                    TEXT NOT NULL,
    -- [Device list] a claim from the device. Browsers hide or freeze parts of it.
    os_version            TEXT NOT NULL,
    -- [Device list] browser name, for web clients only.
    web_platform          TEXT,
    -- [Device list] browser version, for web clients only.
    web_platform_version  TEXT,
    -- [Device list] from proxy headers, else the socket. Kept when the log expires.
    first_ip              TEXT NOT NULL,
    -- [Device list] from proxy headers, else the socket. Kept when the log expires.
    last_ip               TEXT NOT NULL,
    -- [New-device notice, Device list] who is older than whom decides who may remove a notice.
    created_at            INTEGER NOT NULL,
    -- [Reduced view] the last-active day a password-only session is shown.
    last_active_at        INTEGER NOT NULL,
    -- [Termination, Logout, Device limit] set means stopped: no delivery, no longer counted.
    terminated_at         INTEGER,
    -- [Termination, Logout] the signed statement members verify before removing the device
    -- from chats. Signed by the master key, or by this device's own key for a logout.
    revocation            BLOB,
    CHECK ((terminated_at IS NULL) = (revocation IS NULL))
) WITHOUT ROWID, STRICT;

-- [Device limit] `max_devices` counts these rows.
CREATE INDEX device_active ON device (account_id) WHERE terminated_at IS NULL;
-- [Device list, Chat devices] an account's devices, ended ones too.
CREATE INDEX device_account ON device (account_id);

-- [Device log] one row per session. A reconnect within the grace period extends a row.
CREATE TABLE device_session (
    -- [Device log] whose session.
    device_id   BLOB NOT NULL REFERENCES device (id),
    -- [Device log] with WebSocket at connect, with REST at the first activity in the window.
    online_at   INTEGER NOT NULL,
    -- [Device log] NULL while online. The startup sweep fills what a crash left open.
    offline_at  INTEGER,
    -- [Device log] where this session came from, which is what spots a thief.
    ip          TEXT NOT NULL,
    PRIMARY KEY (device_id, online_at)
) WITHOUT ROWID, STRICT;

-- [Device log] at startup the server closes these, because a crash writes no offline time.
CREATE INDEX device_session_open ON device_session (device_id) WHERE offline_at IS NULL;
-- [Device log] the retention sweep.
CREATE INDEX device_session_age ON device_session (online_at);


-- ---------------------------------------------------------------------------
-- RBAC
-- ---------------------------------------------------------------------------

CREATE TABLE role (
    -- [RBAC] what permissions and assignments point at.
    id               BLOB PRIMARY KEY,
    -- [RBAC] 0 server, 1 chat: the two scopes.
    scope            INTEGER NOT NULL CHECK (scope IN (0, 1)),
    -- [RBAC] a chat's own custom role. NULL means available everywhere in its scope.
    conversation_id  BLOB REFERENCES conversation (id) ON DELETE CASCADE,
    -- [RBAC] shown when managing roles.
    name             TEXT NOT NULL,
    -- [RBAC] admin, moderator and member, which cannot be deleted.
    built_in         INTEGER NOT NULL DEFAULT 0 CHECK (built_in IN (0, 1)),
    -- [Device limit] overrides `max_devices` in server.toml. The highest of a person's
    -- roles applies. NULL means this role does not change the limit.
    max_devices      INTEGER,
    CHECK (conversation_id IS NULL OR scope = 1)
) WITHOUT ROWID, STRICT;

-- [RBAC] a name is unique within its chat, or within its scope for shared roles.
CREATE UNIQUE INDEX role_chat_name ON role (conversation_id, name)
    WHERE conversation_id IS NOT NULL;
CREATE UNIQUE INDEX role_shared_name ON role (scope, name)
    WHERE conversation_id IS NULL;

CREATE TABLE role_permission (
    -- [RBAC] which role.
    role_id     BLOB NOT NULL REFERENCES role (id) ON DELETE CASCADE,
    -- [RBAC] the permission's name. A new feature adds names here, not a new mechanism.
    permission  TEXT NOT NULL,
    PRIMARY KEY (role_id, permission)
) WITHOUT ROWID, STRICT;

-- [RBAC] server-scope roles held by an account.
CREATE TABLE account_role (
    -- [RBAC] who holds it.
    account_id  BLOB NOT NULL REFERENCES account (id) ON DELETE CASCADE,
    -- [RBAC] which role.
    role_id     BLOB NOT NULL REFERENCES role (id) ON DELETE CASCADE,
    PRIMARY KEY (account_id, role_id)
) WITHOUT ROWID, STRICT;


-- ---------------------------------------------------------------------------
-- Conversations
-- ---------------------------------------------------------------------------

-- One row per chat, with its last state. Writing a message updates this row, and so does
-- removing or editing a chat's last messages.
CREATE TABLE conversation (
    -- [Everything] what members, messages and keys belong to, and the contents folder's name.
    id                BLOB PRIMARY KEY,
    -- [Chat types, RBAC] 0 self, 1 direct, 2 group, 3 channel.
    type              INTEGER NOT NULL CHECK (type IN (0, 1, 2, 3)),
    -- [Direct chats, Self chat] the lower of the two accounts. A self chat has both equal.
    direct_low        BLOB REFERENCES account (id),
    -- [Direct chats, Self chat] the higher of the two accounts.
    direct_high       BLOB REFERENCES account (id),
    -- [Unencrypted switching] 0 private: everything encrypted, and unencrypted is refused.
    -- 1 mixed: encrypted, but permitted people may send unencrypted messages. 2 open: everything
    -- unencrypted. New chats start as 0.
    -- A copy of what is signed inside MLS: the server uses it to refuse, clients check the
    -- signed one.
    mode              INTEGER NOT NULL DEFAULT 0 CHECK (mode IN (0, 1, 2)),
    -- [Retention] the chat's own limit. NULL falls back to server.toml, and the shortest wins.
    retention_ms      INTEGER,
    -- [Chat list, Sending, Fetch pages] the newest seq. Also what a chat's task resumes from
    -- after going idle, and what a client compares with the last id it has.
    last_seq          INTEGER NOT NULL DEFAULT 0,
    -- [Chat list] the newest message's kind.
    last_kind         INTEGER,
    -- [Chat list] sorts the list by activity. A reaction moves this, so it brings the chat up.
    last_at           INTEGER,
    -- [Chat list] the preview. A reaction moves it, showing "Sara reacted" until the next message.
    -- Commits and a call's own messages do not, and removing or editing the last message does.
    last_content_seq  INTEGER,
    -- [Chat list] sorts a chat with no messages yet.
    created_at        INTEGER NOT NULL,
    CHECK ((type IN (0, 1)) = (direct_low IS NOT NULL)),
    CHECK ((type IN (0, 1)) = (direct_high IS NOT NULL)),
    CHECK (direct_low <= direct_high),
    CHECK ((type = 0) = (direct_low IS NOT NULL AND direct_low = direct_high))
) WITHOUT ROWID, STRICT;

-- [Direct chats, Self chat] one pair cannot get two chats, and a person gets one self chat.
CREATE UNIQUE INDEX conversation_direct ON conversation (direct_low, direct_high)
    WHERE direct_low IS NOT NULL;

-- [Group and channel profile] one row per group or channel, never for direct or self chats.
-- Kept off `conversation` because that row changes on every message. In an encrypted chat the
-- server holds only the sealed blob. In an unencrypted one it can read the fields.
CREATE TABLE conversation_profile (
    -- [Group and channel profile] which chat.
    conversation_id  BLOB PRIMARY KEY REFERENCES conversation (id) ON DELETE CASCADE,
    -- [Group and channel profile, Unencrypted switching] 1 uses sealed and epoch, 0 uses the
    -- readable fields. Switching a chat's mode rewrites this row.
    encrypted        INTEGER NOT NULL CHECK (encrypted IN (0, 1)),
    -- [Encrypted profile] name, bio, links, and image info with its file key, encrypted under
    -- the chat key of `epoch`.
    sealed           BLOB,
    -- [Encrypted profile] the chat key that opens it. Whoever adds a member re-encrypts this
    -- under the new key, because a new member only has the current one.
    epoch            INTEGER,
    -- [Unencrypted profile, Chat list] what the chat is called.
    name             TEXT,
    -- [Unencrypted profile] free text about the chat.
    bio              TEXT,
    -- [Unencrypted profile] the image's file.
    image_file_id    BLOB REFERENCES file (id),
    -- [Unencrypted profile] the image's media type, so it can be served correctly.
    image_type       TEXT,
    -- [Signatures] by updated_by, so a server cannot forge a chat's profile.
    signature        BLOB NOT NULL,
    -- [Signatures, RBAC] which device changed it, to check it held the permission.
    updated_by       BLOB NOT NULL REFERENCES device (id),
    -- [Group and channel profile] lets a client skip fetching what it already has.
    updated_at       INTEGER NOT NULL,
    CHECK ((encrypted = 1) = (sealed IS NOT NULL)),
    CHECK ((encrypted = 1) = (epoch IS NOT NULL)),
    CHECK ((encrypted = 0) = (name IS NOT NULL)),
    CHECK (encrypted = 0 OR (bio IS NULL AND image_file_id IS NULL AND image_type IS NULL)),
    CHECK ((image_file_id IS NULL) = (image_type IS NULL))
) WITHOUT ROWID, STRICT;

-- [Unencrypted profile] an unencrypted group's or channel's links, as key and value. In an
-- encrypted chat, links are inside the sealed blob and this table has no rows.
CREATE TABLE conversation_link (
    -- [Unencrypted profile] which chat.
    conversation_id  BLOB NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
    -- [Unencrypted profile] the label, unique per chat.
    key              TEXT NOT NULL,
    -- [Unencrypted profile] the link itself.
    value            TEXT NOT NULL,
    PRIMARY KEY (conversation_id, key)
) WITHOUT ROWID, STRICT;

-- [Chat list, Read state] who is in a chat, and how far they have read. An account's rows
-- here are its conversation list, which the server works out and the client never sends.
CREATE TABLE member (
    -- [Chat list] which chat.
    conversation_id  BLOB NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
    -- [Chat list] who.
    account_id       BLOB NOT NULL REFERENCES account (id),
    -- [Read state] so a member is never "has not seen" for a message older than their joining.
    joined_seq       INTEGER NOT NULL,
    -- [Read state] the highest "seen up to N". Who has not seen message N: seen_seq < N and
    -- joined_seq <= N, except its sender. Replaces a row per message per member.
    seen_seq         INTEGER NOT NULL DEFAULT 0,
    -- [Read state, Marks] the highest "received up to N": a device of this member holds those
    -- messages. Drives the second mark in a direct chat.
    received_seq     INTEGER NOT NULL DEFAULT 0,
    -- [Owners, Unencrypted switching, RBAC] groups and channels can have several owners, and
    -- ownership can be transferred. Recorded inside MLS too, which is what clients trust, so a
    -- server cannot make someone an owner. When the last owner leaves, the chat is removed.
    is_owner         INTEGER NOT NULL DEFAULT 0 CHECK (is_owner IN (0, 1)),
    -- [Remove conversation] direct chats only: hidden from this account, as of this seq, and from
    -- nobody else. NULL means not hidden. The next message clears it and everything comes back,
    -- except messages already removed. If both members have it, the chat and all its rows go, so
    -- the next message starts a new chat. Groups and channels use leaving instead.
    removed_seq      INTEGER,
    PRIMARY KEY (conversation_id, account_id)
) WITHOUT ROWID, STRICT;

-- [Chat list] the list and state for all of a person's chats, in one query.
CREATE INDEX member_account ON member (account_id, conversation_id);
-- [Owners] whether someone leaving is the last owner.
CREATE INDEX member_owner ON member (conversation_id) WHERE is_owner = 1;
-- [Read state, Seen counts] who has or has not seen message N, and how many.
CREATE INDEX member_seen ON member (conversation_id, seen_seq);

-- [RBAC] chat-scope roles held by a member.
CREATE TABLE member_role (
    -- [RBAC] which chat.
    conversation_id  BLOB NOT NULL,
    -- [RBAC] who holds it.
    account_id       BLOB NOT NULL,
    -- [RBAC] which role.
    role_id          BLOB NOT NULL REFERENCES role (id) ON DELETE CASCADE,
    PRIMARY KEY (conversation_id, account_id, role_id),
    FOREIGN KEY (conversation_id, account_id)
        REFERENCES member (conversation_id, account_id) ON DELETE CASCADE
) WITHOUT ROWID, STRICT;


-- ---------------------------------------------------------------------------
-- Messages
-- ---------------------------------------------------------------------------

-- Whatever is saved to show later. Typing and stop typing never reach this table.
-- A removed message, whether deleted or expired, keeps only conversation_id and seq, with every
-- other field NULL. So pages have no gaps, and a client knows something was there, not what.
CREATE TABLE message (
    -- [Sending, Fetch pages] which chat.
    conversation_id    BLOB NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
    -- [Sending, Fetch pages, Pointers] from 1, assigned by the chat's task. Decides order.
    seq                INTEGER NOT NULL,
    -- [Signatures, Device list, New-device notice] which device sent it, which also gives the
    -- account. For a device_notice this is the new device, and for its first 7 days the server
    -- refuses a removal from it or any device newer than it.
    device_id          BLOB REFERENCES device (id),
    -- [RBAC, Chat list, Actions] always readable, because RBAC needs it:
    -- 0 content, 1 edit, 2 delete, 3 commit, 4 device_notice.
    -- A reply is not a kind: it is a content message with a pointer.
    -- A device_notice is written by the server, in the person's self chat only.
    kind               INTEGER CHECK (kind BETWEEN 0 AND 4),
    -- [RBAC, Chat list, Calls] what a content message is, and NULL for every other kind. Readable
    -- by the server even in an encrypted chat, so roles can allow or refuse each one:
    -- 0 text, 1 voice, 2 reaction, 3 image, 4 video, 5 doc, 6 file,
    -- 7 call (one per call, saved, and can be replied to; what happens inside it is in
    -- call_event), 8 media_bundle (several images and videos in one message).
    content_kind       INTEGER CHECK (content_kind BETWEEN 0 AND 8),
    -- [Unencrypted messages] the flag that tells a client how to read the body.
    encrypted          INTEGER CHECK (encrypted IN (0, 1)),
    -- [Restore, Commit order] the key this was encrypted under, and for a commit, the step it
    -- claims. NULL for an unencrypted message that is not a commit.
    epoch              INTEGER,
    -- [Actions, Replies] the target's seq. In an encrypted chat only delete and edit fill it,
    -- because the server acts on those. Reply and reaction pointers stay inside the ciphertext.
    pointer_seq        INTEGER,
    -- [Calls, Chat list] for a call: 0 missed, 1 answered, NULL while it is ringing. Written by the
    -- server. Readable because a missed call counts as unread and an answered one does not.
    call_outcome       INTEGER CHECK (call_outcome IN (0, 1)),
    -- [Calls] when a call ended, written by the server. NULL while it can still be joined.
    call_ended_at      INTEGER,
    -- [Resend check] the client's clock.
    client_sent_at     INTEGER,
    -- [Chat list, Retention] the server's clock.
    server_saved_at    INTEGER,
    -- [Retention] NULL means never. Always NULL for a device_notice, which retention never touches.
    expires_at         INTEGER,
    -- [Signatures, New-device notice] by device_id, encrypted or not, so clients detect forgery
    -- by a server. For a device_notice this is a copy of the device's master_stamp: the server
    -- writes that row, but only the words can have made the stamp.
    signature          BLOB,
    -- [Sending, Restore] ciphertext, or plaintext if `encrypted` is 0. Empty for a
    -- device_notice, whose details are on the device row.
    body               BLOB,
    PRIMARY KEY (conversation_id, seq),
    CHECK (encrypted = 0 OR epoch IS NOT NULL),
    CHECK ((kind = 0) = (content_kind IS NOT NULL)),
    CHECK (call_outcome IS NULL OR content_kind = 7),
    CHECK (call_ended_at IS NULL OR content_kind = 7),
    CHECK (kind <> 3 OR epoch IS NOT NULL),
    CHECK (kind NOT IN (1, 2) OR pointer_seq IS NOT NULL),
    CHECK (content_kind IS NOT 2 OR encrypted = 1 OR pointer_seq IS NOT NULL),
    CHECK (kind <> 4 OR encrypted = 0),
    CHECK (kind <> 4 OR expires_at IS NULL),
    -- A row is either a whole message or only its seq, never partly wiped.
    CHECK ((kind IS NULL) = (device_id IS NULL)),
    CHECK ((kind IS NULL) = (encrypted IS NULL)),
    CHECK ((kind IS NULL) = (client_sent_at IS NULL)),
    CHECK ((kind IS NULL) = (server_saved_at IS NULL)),
    CHECK ((kind IS NULL) = (signature IS NULL)),
    CHECK ((kind IS NULL) = (body IS NULL)),
    CHECK (kind IS NOT NULL OR (epoch IS NULL AND pointer_seq IS NULL AND call_outcome IS NULL AND call_ended_at IS NULL AND expires_at IS NULL))
) STRICT;

-- [Chat list] finds the preview after the last message is removed.
CREATE INDEX message_preview ON message (conversation_id, seq) WHERE kind = 0;
-- [Chat list] counts unread. A reaction does not count, nor an answered call. A missed call does.
CREATE INDEX message_unread ON message (conversation_id, seq)
    WHERE kind = 0 AND content_kind <> 2 AND (call_outcome IS NULL OR call_outcome = 0);
-- [Commit order] the first commit for an epoch wins, and a second cannot be stored.
CREATE UNIQUE INDEX message_commit ON message (conversation_id, epoch) WHERE kind = 3;
-- [Delete] an edit holds a message's newer text, so deleting the message must find and remove
-- its edits too. Edit pointers are readable, which is what makes this possible.
CREATE INDEX message_edit ON message (conversation_id, pointer_seq) WHERE kind = 1;
-- [Calls] calls still open, ended at startup since nobody can be in them after a restart.
CREATE INDEX message_open_call ON message (conversation_id, seq)
    WHERE content_kind = 7 AND call_ended_at IS NULL;
-- [Retention] the sweep.
CREATE INDEX message_expiry ON message (expires_at) WHERE expires_at IS NOT NULL;

-- [Call replay] what happened inside a call, never chat rows: joins, leaves and the end written by
-- the server, and declines and text from clients.
CREATE TABLE call_event (
    -- [Calls] the call's chat.
    conversation_id    BLOB NOT NULL,
    -- [Calls] the sequence id of the call's message.
    call_seq           INTEGER NOT NULL,
    -- [Calls] 1, 2, 3 inside the call, in the order the chat's task saved them.
    n                  INTEGER NOT NULL CHECK (n >= 1),
    -- [Calls] 0 join, 1 leave, 2 decline, 3 text, 4 end. Join, leave and end are the server's.
    kind               INTEGER NOT NULL CHECK (kind BETWEEN 0 AND 4),
    -- [Calls] whose. For an end, the device whose leaving or decline ended it, or the caller's if it
    -- rang out or was still open at startup.
    device_id          BLOB NOT NULL REFERENCES device (id),
    -- [Calls] the server's clock.
    server_at          INTEGER NOT NULL,
    -- [Calls, Signatures] text only: the client's clock, the key it is sealed under, its signature
    -- over the header and body, and the ciphertext. The header names call_seq, so it can never
    -- pass for a chat message.
    client_at          INTEGER,
    epoch              INTEGER,
    signature          BLOB,
    body               BLOB,
    PRIMARY KEY (conversation_id, call_seq, n),
    FOREIGN KEY (conversation_id, call_seq) REFERENCES message (conversation_id, seq)
        ON DELETE CASCADE,
    CHECK ((kind = 3) = (body IS NOT NULL)),
    CHECK ((kind = 3) = (signature IS NOT NULL)),
    CHECK ((kind = 3) = (epoch IS NOT NULL)),
    CHECK ((kind = 3) = (client_at IS NOT NULL))
) WITHOUT ROWID, STRICT;

-- [Seen times] one row per `seen`, with one time for many messages. A member saw message N
-- at the time on their first row reaching N. Deleted once the messages it covers expire.
-- Not tied to `member`: a member who leaves keeps their seen times, so the code checks
-- membership before writing one.
CREATE TABLE seen (
    -- [Seen times] which chat.
    conversation_id  BLOB NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
    -- [Seen times] who.
    account_id       BLOB NOT NULL REFERENCES account (id),
    -- [Seen times, Read state] the "seen up to N" the client sent.
    up_to_seq        INTEGER NOT NULL,
    -- [Devices] which device reported it.
    device_id        BLOB NOT NULL REFERENCES device (id),
    -- [Seen times] accurate for reading offline, shown unless later than the server's or far
    -- from it.
    client_seen_at   INTEGER NOT NULL,
    -- [Seen times] catches a client lying about its time.
    server_seen_at   INTEGER NOT NULL,
    PRIMARY KEY (conversation_id, account_id, up_to_seq)
) WITHOUT ROWID, STRICT;

-- [Received times] one row per `received`, like `seen`: a member's device got message N at the time
-- on their first row reaching N. Deleted once the messages it covers expire. Not tied to `member`,
-- so a member who leaves keeps their received times.
CREATE TABLE received (
    -- [Received times] which chat.
    conversation_id     BLOB NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
    -- [Received times] who.
    account_id          BLOB NOT NULL REFERENCES account (id),
    -- [Received times, Read state] the "received up to N" the client sent.
    up_to_seq           INTEGER NOT NULL,
    -- [Devices] which device reported it.
    device_id           BLOB NOT NULL REFERENCES device (id),
    -- [Received times] the device's own time, shown unless later than the server's or far from it.
    client_received_at  INTEGER NOT NULL,
    -- [Received times] catches a client lying about its time.
    server_received_at  INTEGER NOT NULL,
    PRIMARY KEY (conversation_id, account_id, up_to_seq)
) WITHOUT ROWID, STRICT;


-- ---------------------------------------------------------------------------
-- Files
-- ---------------------------------------------------------------------------

-- [Files, Profile images] what the server knows about a stored file, which sits in its chat's
-- or account's folder. Its name, type and key are inside an encrypted message or profile, so
-- the server sees only an unnamed blob, its exact size and who uploaded it. The server does not
-- know which message names which file: a client lists the file ids with its delete or edit.
CREATE TABLE file (
    -- [Files] names the file on disk, and what a client asks for.
    id               BLOB PRIMARY KEY,
    -- [Files, Retention] the chat it belongs to. NULL for an account's profile image.
    conversation_id  BLOB REFERENCES conversation (id) ON DELETE CASCADE,
    -- [Profile images] the account it belongs to. NULL for a chat's file.
    account_id       BLOB REFERENCES account (id) ON DELETE CASCADE,
    -- [Unencrypted messages] 0 only for files in an unencrypted chat.
    encrypted        INTEGER NOT NULL CHECK (encrypted IN (0, 1)),
    -- [Upload limits] the type its uploader declared, which picks the limit: 1 voice, 3 image,
    -- 4 video, 5 doc, 6 file, as message.content_kind. The real type is encrypted.
    kind             INTEGER NOT NULL CHECK (kind IN (1, 3, 4, 5, 6)),
    -- [Quotas, Upload limits] the exact encrypted size in bytes.
    size             INTEGER NOT NULL CHECK (size >= 0),
    -- [Files] how many chunks it is uploaded in.
    chunks           INTEGER NOT NULL CHECK (chunks >= 1),
    -- [Files] how many chunks arrived, in order, so an upload resumes from there.
    received         INTEGER NOT NULL DEFAULT 0 CHECK (received BETWEEN 0 AND chunks),
    -- [Quotas, Signatures, Delete] which device uploaded it. Its account may remove the file, and
    -- so may a role permitted to in that chat, and either person for a call's recording.
    uploaded_by      BLOB NOT NULL REFERENCES device (id),
    -- [Retention] when it arrived.
    created_at       INTEGER NOT NULL,
    -- [Unused uploads] set when a client lists this file id with a message it sends. Still NULL
    -- after a while means it was never sent, so it gets removed.
    used_at          INTEGER,
    -- [Retention] NULL means never. A file has its own expiry because nobody names the files of
    -- a message that expires. Set a little after the limit, since a file uploads before its
    -- message is sent.
    expires_at       INTEGER,
    -- [Delete, Retention] set when the bytes are gone and the record stays.
    removed_at       INTEGER,
    CHECK ((conversation_id IS NULL) <> (account_id IS NULL))
) WITHOUT ROWID, STRICT;

-- [Files, Quotas] a chat's files.
CREATE INDEX file_conversation ON file (conversation_id) WHERE conversation_id IS NOT NULL;
-- [Profile images, Quotas] an account's files.
CREATE INDEX file_account ON file (account_id) WHERE account_id IS NOT NULL;
-- [Unused uploads] checked when a chat's task loads, and for every chat at startup. There is no
-- periodic job.
CREATE INDEX file_unused ON file (conversation_id, created_at) WHERE used_at IS NULL;
-- [Retention] the sweep.
CREATE INDEX file_expiry ON file (expires_at) WHERE expires_at IS NOT NULL;


-- ---------------------------------------------------------------------------
-- MLS and keys
-- ---------------------------------------------------------------------------

-- [External join, Commit order] every chat has one, even while unencrypted. Kept off
-- `conversation` because group info can be large and that row changes on every message.
CREATE TABLE mls_group (
    -- [External join] which chat.
    conversation_id  BLOB PRIMARY KEY REFERENCES conversation (id) ON DELETE CASCADE,
    -- [Commit order] a commit must be for this epoch, or it lost the race and must retry.
    epoch            INTEGER NOT NULL,
    -- [External join] public, and what a new device joins the chat with by itself.
    group_info       BLOB NOT NULL,
    -- [External join] lets a client skip fetching group info it already has.
    updated_at       INTEGER NOT NULL
) STRICT;

-- [Restore, Key wrapping] one epoch's key, wrapped to one member account by whoever changed
-- the key. This is what lets a new device read everything with only the words.
CREATE TABLE epoch_key (
    -- [Restore] which chat.
    conversation_id  BLOB NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
    -- [Restore] matched against message.epoch.
    epoch            INTEGER NOT NULL,
    -- [Restore] which account can unwrap it.
    account_id       BLOB NOT NULL REFERENCES account (id),
    -- [Restore] about 64 bytes, opened only with the words.
    wrapped_key      BLOB NOT NULL,
    -- [Key wrapping] who wrapped it, to trace a badly wrapped key.
    wrapped_by       BLOB NOT NULL REFERENCES device (id),
    PRIMARY KEY (conversation_id, epoch, account_id)
) WITHOUT ROWID, STRICT;

-- [Restore] all of an account's keys for a new device.
CREATE INDEX epoch_key_account ON epoch_key (account_id);

-- [First contact] what other people use to add this device to a chat. Used once, then deleted.
CREATE TABLE key_package (
    -- [First contact] which package was taken.
    id          BLOB PRIMARY KEY,
    -- [First contact] whose device it adds.
    device_id   BLOB NOT NULL REFERENCES device (id),
    -- [First contact] the MLS KeyPackage.
    body        BLOB NOT NULL,
    -- [First contact] replacing old ones.
    created_at  INTEGER NOT NULL
) STRICT;

-- [First contact]
CREATE INDEX key_package_device ON key_package (device_id);

-- [First contact] an MLS Welcome waiting for a device that was added to a chat.
CREATE TABLE mls_welcome (
    -- [First contact] which one the device has picked up.
    id               BLOB PRIMARY KEY,
    -- [First contact] who it is for.
    device_id        BLOB NOT NULL REFERENCES device (id),
    -- [First contact] which chat it joins.
    conversation_id  BLOB NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
    -- [First contact] the MLS Welcome.
    body             BLOB NOT NULL,
    -- [First contact] clearing ones never picked up.
    created_at       INTEGER NOT NULL
) STRICT;

-- [First contact]
CREATE INDEX mls_welcome_device ON mls_welcome (device_id);

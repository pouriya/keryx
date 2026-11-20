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
    -- [Invitations] what an invitation's name and BlurHash are sealed to, with the master key's
    -- stamp over it, so a server cannot read an invitation by swapping in its own key. NULL until
    -- the words are set up.
    invite_public_key   BLOB,
    invite_key_stamp    BLOB,
    -- [Words setup] when setup finished, so the client knows whether to ask for it.
    words_set_at        INTEGER,
    -- [Account profile] moves on every change of the names, photo, bio or links, so a client that
    -- was offline knows to fetch the profile again.
    profile_version     INTEGER NOT NULL DEFAULT 0 CHECK (profile_version >= 0),
    -- [Settings] moves on every change to the account's `kv` rows, so a device that holds this
    -- version is answered with nothing, and a change based on an older one is refused.
    kv_version          INTEGER NOT NULL DEFAULT 0 CHECK (kv_version >= 0),
    -- [Agents] the person who owns this agent. NULL for people. An owner is never an agent itself.
    owner_id            BLOB REFERENCES account (id),
    -- [Agents] when its owner disabled it. It stays for old signatures, and stops counting toward
    -- `[limits] agents`. NULL for people and running agents.
    disabled_at         INTEGER,
    -- [Admin] when the account was made.
    created_at          INTEGER NOT NULL
) WITHOUT ROWID, STRICT;

-- [Agents] counting an owner's agents, and listing them.
CREATE INDEX account_owner ON account (owner_id) WHERE owner_id IS NOT NULL;

-- [Agents] an agent's one token. A login's JWT must carry this row's jti, so a renew, which replaces
-- it, or a revoke, which deletes it, ends the old token at once.
CREATE TABLE agent_token (
    -- [Agents] whose token.
    account_id  BLOB PRIMARY KEY REFERENCES account (id) ON DELETE CASCADE,
    -- [Agents] the token's id, 16 random bytes. Not secret: the JWT's signature is what the
    -- secret in `[agents]` guards.
    jti         BLOB NOT NULL CHECK (length(jti) = 16),
    -- [Agents] when it was made, shown to its owner.
    created_at  INTEGER NOT NULL
) WITHOUT ROWID, STRICT;

-- [Agents] the server permissions an owner ticked for its agent. The agent holds each only while its
-- owner does too, checked on every operation, and never `agent.create`.
CREATE TABLE agent_permission (
    -- [Agents] which agent.
    account_id  BLOB NOT NULL REFERENCES account (id) ON DELETE CASCADE,
    -- [Agents] a server permission's name, from the closed list in `api.rs`.
    permission  TEXT NOT NULL,
    PRIMARY KEY (account_id, permission)
) WITHOUT ROWID, STRICT;

-- [Account profile] one encrypted blob per account: the photo's id and file key, the bio, and
-- links as name and address, under the profile key the words give. Readable only by people sharing
-- a chat with this account.
CREATE TABLE account_profile (
    -- [Account profile] whose profile.
    account_id  BLOB PRIMARY KEY REFERENCES account (id) ON DELETE CASCADE,
    -- [Account profile] the blob, which the server cannot read.
    sealed      BLOB NOT NULL,
    -- [Signatures] by updated_by, so a server cannot forge someone's profile.
    signature   BLOB NOT NULL,
    -- [Signatures] which device changed it.
    updated_by  BLOB NOT NULL REFERENCES device (id),
    -- [Profile images] the photo the blob names, an account file. NULL with no photo, or once
    -- admin removed it.
    photo_id    BLOB REFERENCES file (id) ON DELETE SET NULL,
    -- [Account profile] when it last changed.
    updated_at  INTEGER NOT NULL
) STRICT;

-- [Account profile] an account's profile key, shared with one chat under that chat's key, the newest
-- only. This is one row per chat, not one profile per chat, so changing a profile rewrites one blob.
-- The key comes from the words and does not rotate when someone leaves, and channels get no rows
-- here.
CREATE TABLE account_profile_key (
    -- [Account profile] whose profile this opens.
    account_id       BLOB NOT NULL REFERENCES account (id) ON DELETE CASCADE,
    -- [Account profile] which chat's members can open it.
    conversation_id  BLOB NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
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
    -- [Password reset, Returning device, Device ids, Folders, Topics] 32 random bytes, or a
    -- device's, folder's or topic's ULID.
    value       BLOB PRIMARY KEY,
    -- [Password reset, Returning device, Device ids, Folders, Topics] which account it is for.
    account_id  BLOB NOT NULL REFERENCES account (id) ON DELETE CASCADE,
    -- [Password reset, Returning device, Device ids, Folders, Topics] 0 password reset, 1 returning
    -- device, 2 device id, 3 folder id, 4 topic id: so one cannot be used as another.
    purpose     INTEGER NOT NULL CHECK (purpose IN (0, 1, 2, 3, 4)),
    -- [Topics] the chat a topic id was issued for. NULL for every other purpose.
    conversation_id  BLOB REFERENCES conversation (id) ON DELETE CASCADE,
    -- [Password reset, Returning device, Device ids, Folders, Topics] five minutes after it was made.
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
    -- [Presence] when it last had a connection that had announced itself: written at its first
    -- `presence` and when that connection ended. Not taken from device_session, whose rows expire.
    last_connected_at     INTEGER,
    -- [Presence] when its person last had keryx open on screen: written on every app open and
    -- close, and when a connection with the app open ended.
    last_in_app_at        INTEGER,
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
    -- [RBAC] the server's admin, moderator and member, and a chat's owner, moderator and member,
    -- which cannot be deleted.
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
    -- [Topics] whether a group's or channel's messages show in named topics. Off shows one list,
    -- and every message keeps its topic.
    topics_on         INTEGER NOT NULL DEFAULT 0 CHECK (topics_on IN (0, 1)),
    -- [Findable chats] a group or channel the directory lists, whose name and bio the server reads.
    findable          INTEGER NOT NULL DEFAULT 0 CHECK (findable IN (0, 1)),
    -- [Findable chats] its readable name, set only while findable.
    find_name         TEXT,
    -- [Findable chats] its readable bio.
    find_bio          TEXT,
    -- [Slow mode] seconds between one member's messages in the main list or a topic. NULL is off.
    slow_mode         INTEGER CHECK (slow_mode IN (10, 30, 60, 300, 900, 3600)),
    -- [Retention] the chat's own limit. NULL falls back to server.toml, and the shortest wins.
    retention_ms      INTEGER,
    -- [Chat list, Sending, Fetch pages] the newest seq. Also what a chat's task resumes from
    -- after going idle, and what a client compares with the last id it has.
    last_seq          INTEGER NOT NULL DEFAULT 0,
    -- [Chat list] the newest message's kind.
    last_kind         INTEGER,
    -- [Chat list] sorts the list by activity. A reaction moves this, so it brings the chat up. An
    -- ack does not.
    last_at           INTEGER,
    -- [Chat list] the preview. A reaction moves it, showing "Sara reacted" until the next message,
    -- and so does a membership line.
    -- Commits, acks and a call's own messages do not, and removing or editing the last message does.
    last_content_seq  INTEGER,
    -- [Chat list] sorts a chat with no messages yet.
    created_at        INTEGER NOT NULL,
    CHECK ((type IN (0, 1)) = (direct_low IS NOT NULL)),
    CHECK ((type IN (0, 1)) = (direct_high IS NOT NULL)),
    CHECK (direct_low <= direct_high),
    CHECK ((type = 0) = (direct_low IS NOT NULL AND direct_low = direct_high)),
    CHECK (type IN (2, 3) OR (topics_on = 0 AND findable = 0 AND slow_mode IS NULL)),
    CHECK ((findable = 1) = (find_name IS NOT NULL)),
    CHECK (findable = 1 OR find_bio IS NULL)
) WITHOUT ROWID, STRICT;

-- [Findable chats] the directory's search.
CREATE INDEX conversation_findable ON conversation (find_name) WHERE findable = 1;

-- [Direct chats, Self chat] one pair cannot get two chats, and a person gets one self chat.
CREATE UNIQUE INDEX conversation_direct ON conversation (direct_low, direct_high)
    WHERE direct_low IS NOT NULL;

-- [Group and channel profile] one row per group or channel, never for direct or self chats.
-- Kept off `conversation` because that row changes on every message. The server holds only the
-- sealed blob.
CREATE TABLE conversation_profile (
    -- [Group and channel profile] which chat.
    conversation_id  BLOB PRIMARY KEY REFERENCES conversation (id) ON DELETE CASCADE,
    -- [Group and channel profile, Topics] name, bio, links, the photo with its file key, and the
    -- topics' names, encrypted under the chat's own profile key, which never changes.
    sealed           BLOB NOT NULL,
    -- [Group and channel profile, Invitations] that profile key, encrypted under the chat key of
    -- `key_epoch`. Its creator, an add and a device joining by invitation share it again at their
    -- epoch, so every member holds the key of the newest.
    key              BLOB NOT NULL,
    -- [Group and channel profile, Invitations] the epoch whose chat key opens `key`.
    key_epoch        INTEGER NOT NULL,
    -- [Group and channel profile] the photo the blob names, a chat file. NULL with no photo.
    photo_id         BLOB REFERENCES file (id) ON DELETE SET NULL,
    -- [Signatures] by updated_by, so a server cannot forge a chat's profile.
    signature        BLOB NOT NULL,
    -- [Signatures, RBAC] which device changed it, to check it held the permission.
    updated_by       BLOB NOT NULL REFERENCES device (id),
    -- [Group and channel profile] moves on every change, so a device fetches only a newer one.
    version          INTEGER NOT NULL CHECK (version >= 1),
    -- [Group and channel profile] when it last changed.
    updated_at       INTEGER NOT NULL
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
    -- [Owners, RBAC] groups and channels can have several owners, and
    -- ownership can be transferred. Recorded inside MLS too, which is what clients trust, so a
    -- server cannot make someone an owner. When the last owner leaves, the chat is removed.
    is_owner         INTEGER NOT NULL DEFAULT 0 CHECK (is_owner IN (0, 1)),
    -- [Remove conversation] direct chats only: hidden from this account, as of this seq, and from
    -- nobody else. NULL means not hidden. The next message clears it and everything comes back,
    -- except messages already removed. If both members have it, the chat and all its rows go, so
    -- the next message starts a new chat. Groups and channels use leaving instead.
    removed_seq      INTEGER,
    -- [Drafts] this account's unsent text in the chat, sealed by its client under the chat's key at
    -- `draft_epoch`. The server never reads it. NULL means no draft.
    draft            BLOB,
    -- [Drafts] the epoch the draft is sealed at.
    draft_epoch      INTEGER,
    -- [Drafts] the server time it was saved, so a device keeps the newer one.
    draft_at         INTEGER,
    CHECK ((draft IS NULL) = (draft_epoch IS NULL) AND (draft IS NULL) = (draft_at IS NULL)),
    PRIMARY KEY (conversation_id, account_id)
) WITHOUT ROWID, STRICT;

-- [Chat list] the list and state for all of a person's chats, in one query.
CREATE INDEX member_account ON member (account_id, conversation_id);
-- [Owners] whether someone leaving is the last owner.
CREATE INDEX member_owner ON member (conversation_id) WHERE is_owner = 1;
-- [Read state, Seen counts] who has or has not seen message N, and how many.
CREATE INDEX member_seen ON member (conversation_id, seen_seq);

-- [Folders] an account's arrangement of its chats, kept here so it follows the account to every
-- device. Saved whole each time. `all` is a row too, written with the account, and holds every chat.
-- Not WITHOUT ROWID: `chats` grows with the account's chats.
CREATE TABLE folder (
    -- [Folders] issued by the server first, since the sealed name's associated data holds it.
    id          BLOB PRIMARY KEY,
    -- [Folders] whose.
    account_id  BLOB NOT NULL REFERENCES account (id) ON DELETE CASCADE,
    -- [Folders] tab order, from 0.
    place       INTEGER NOT NULL CHECK (place >= 0),
    -- [Folders] sealed by the account's client under its profile key. The server never reads it.
    -- NULL only for `all`.
    name        BLOB,
    -- [Folders] its chat ids, 16 bytes each. NULL only for `all`, which holds every chat.
    chats       BLOB CHECK (length(chats) % 16 = 0),
    -- [Pins] its pinned chat ids, 16 bytes each, top first.
    pins        BLOB NOT NULL CHECK (length(pins) % 16 = 0),
    -- [Folders] the tab a device opens on.
    is_default  INTEGER NOT NULL CHECK (is_default IN (0, 1)),
    CHECK ((name IS NULL) = (chats IS NULL))
) STRICT;

-- [Folders] one place, one default and one `all` per account.
CREATE UNIQUE INDEX folder_place ON folder (account_id, place);
CREATE UNIQUE INDEX folder_default ON folder (account_id) WHERE is_default = 1;
CREATE UNIQUE INDEX folder_all ON folder (account_id) WHERE name IS NULL;

-- [Settings] an account's key-value store, so every device shows the same settings. Values are
-- bytes the server never reads: the client decides what is sealed.
CREATE TABLE kv (
    -- [Settings] whose.
    account_id  BLOB NOT NULL REFERENCES account (id) ON DELETE CASCADE,
    -- [Settings] a-z, 0-9, _ and ., 1 to 64 long.
    key         TEXT NOT NULL CHECK (
                    length(key) BETWEEN 1 AND 64 AND key NOT GLOB '*[^a-z0-9_.]*'
                ),
    -- [Settings] at most `[limits.body] kv_value`, checked in the code.
    value       BLOB NOT NULL,
    PRIMARY KEY (account_id, key)
) WITHOUT ROWID, STRICT;

-- [RBAC] chat-scope roles held by a member: a moderator's row. Owners are `member.is_owner`, and
-- every member holds the built-in member role without a row.
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

-- [Chat options] a chat's changes to the built-in member role: reactions, stickers, files and media,
-- and whether members may add people. A row adds the permission to its members or takes it away.
CREATE TABLE chat_permission (
    -- [Chat options] which chat.
    conversation_id  BLOB NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
    -- [Chat options] the permission's name, with the chat type's prefix.
    permission       TEXT NOT NULL,
    -- [Chat options] 1 added to the member role, 0 taken from it.
    allowed          INTEGER NOT NULL CHECK (allowed IN (0, 1)),
    PRIMARY KEY (conversation_id, permission)
) WITHOUT ROWID, STRICT;

-- [Topics] a group's or channel's topics, their names inside the sealed profile. General's own
-- settings sit in a row under the all-zero id, made with the chat, while its messages carry no topic.
CREATE TABLE topic (
    -- [Topics] which chat.
    conversation_id  BLOB NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
    -- [Topics] issued by the server first, so the sealed name can name it. All zeros for General.
    id               BLOB NOT NULL,
    -- [Topics] when its id was issued.
    issued_at        INTEGER NOT NULL,
    -- [Topics] set once removed: its messages are wiped and nothing new goes in. The row stays, so a
    -- removed id is never live again.
    removed          INTEGER NOT NULL DEFAULT 0 CHECK (removed IN (0, 1)),
    -- [Topic settings] nobody but moderators writes, reacts or replies in it.
    closed           INTEGER NOT NULL DEFAULT 0 CHECK (closed IN (0, 1)),
    -- [Topic settings] only moderators see it, and its messages go to nobody else.
    hidden           INTEGER NOT NULL DEFAULT 0 CHECK (hidden IN (0, 1)),
    -- [Topic settings] who writes in its main list: 0 everyone, 1 moderators.
    write            INTEGER NOT NULL DEFAULT 0 CHECK (write IN (0, 1)),
    -- [Topic settings] seconds between one member's messages in it. NULL follows the chat's, 0 is off.
    slow_mode        INTEGER CHECK (slow_mode IN (0, 10, 30, 60, 300, 900, 3600)),
    -- [Topic settings] its bio, links, photo and rules, sealed under the chat's profile key.
    sealed           BLOB,
    -- [Topic settings] the sealing device's signature over it.
    signature        BLOB,
    -- [Topic settings] the chat file its photo is, kept while named.
    photo_id         BLOB,
    -- [Topic settings] the device that sealed it.
    updated_by       BLOB,
    -- [Topic settings] 1, 2, 3 as it changes.
    version          INTEGER NOT NULL DEFAULT 0,
    -- [Topic settings] when its profile last changed.
    updated_at       INTEGER,
    PRIMARY KEY (conversation_id, id),
    CHECK ((sealed IS NULL) = (signature IS NULL))
) WITHOUT ROWID, STRICT;

-- [Slow mode] when each member last wrote in each topic's main list, General under the all-zero id,
-- so a topic's slow mode counts only messages in that topic.
CREATE TABLE slow_sent (
    -- [Slow mode] which chat.
    conversation_id  BLOB NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
    -- [Slow mode] who wrote.
    account_id       BLOB NOT NULL,
    -- [Slow mode] the topic written in. All zeros for General.
    topic_id         BLOB NOT NULL,
    -- [Slow mode] the server time of that member's last message there.
    at               INTEGER NOT NULL,
    PRIMARY KEY (conversation_id, topic_id, account_id)
) WITHOUT ROWID, STRICT;

-- [Topic settings] a topic's own override of the member role, on top of the chat's: the reaction,
-- sticker and file kinds and the four times permissions.
CREATE TABLE topic_permission (
    -- [Topic settings] which chat.
    conversation_id  BLOB NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
    -- [Topic settings] which topic, all zeros for General.
    topic_id         BLOB NOT NULL,
    -- [Topic settings] the permission's name, with the chat type's prefix.
    permission       TEXT NOT NULL,
    -- [Topic settings] 1 added to the member role there, 0 taken from it.
    allowed          INTEGER NOT NULL CHECK (allowed IN (0, 1)),
    PRIMARY KEY (conversation_id, topic_id, permission)
) WITHOUT ROWID, STRICT;

-- [Topic settings] a topic's own moderators, made by the chat's owners. The chat's owners and
-- moderators moderate every topic anyway.
CREATE TABLE topic_moderator (
    -- [Topic settings] which chat.
    conversation_id  BLOB NOT NULL,
    -- [Topic settings] which topic, all zeros for General.
    topic_id         BLOB NOT NULL,
    -- [Topic settings] who.
    account_id       BLOB NOT NULL,
    PRIMARY KEY (conversation_id, topic_id, account_id),
    FOREIGN KEY (conversation_id, account_id)
        REFERENCES member (conversation_id, account_id) ON DELETE CASCADE
) WITHOUT ROWID, STRICT;

-- [Join requests] someone asking to join a findable group or channel. Let in by an add commit, which
-- deletes the row. Expires after `[limits] invitation_lifetime`.
CREATE TABLE join_request (
    -- [Join requests] which chat.
    conversation_id  BLOB NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
    -- [Join requests] who asks.
    account_id       BLOB NOT NULL REFERENCES account (id) ON DELETE CASCADE,
    -- [Join requests] plain text the server reads, at most 140 characters.
    note             TEXT,
    -- [Join requests] 0 waiting, 1 declined: the asker is told, and may ask again.
    state            INTEGER NOT NULL CHECK (state IN (0, 1)),
    -- [Join requests, Expiry] when asked or declined.
    at               INTEGER NOT NULL,
    PRIMARY KEY (conversation_id, account_id)
) WITHOUT ROWID, STRICT;

-- [Invitations] an add turned into an invitation, as the invited person's switch says. Deleted when
-- answered or expired.
CREATE TABLE invitation (
    -- [Invitations] which chat.
    conversation_id  BLOB NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
    -- [Invitations] who is invited.
    account_id       BLOB NOT NULL REFERENCES account (id) ON DELETE CASCADE,
    -- [Invitations] which device invited, which gives the inviter and the key its signature checks.
    inviter          BLOB NOT NULL REFERENCES device (id),
    -- [Invitations] the chat's name, its photo's BlurHash, its profile key and the inviter's note,
    -- sealed to the invited account.
    sealed           BLOB NOT NULL,
    -- [Invitations] the inviter's device over `keryx v1 invite`, the chat and the invited account,
    -- which the joiner hands to the chat's members.
    signature        BLOB NOT NULL,
    -- [Invitations, Expiry] when invited.
    at               INTEGER NOT NULL,
    PRIMARY KEY (conversation_id, account_id)
) WITHOUT ROWID, STRICT;

-- [Invitations] an account's own invitations, for its chat list.
CREATE INDEX invitation_account ON invitation (account_id);
-- [Join requests] an account's own requests, for its chat list.
CREATE INDEX join_request_account ON join_request (account_id);


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
    -- 0 content, 1 edit, 2 delete, 3 commit, 4 device_notice, 5 line.
    -- A reply is not a kind: it is a content message with a pointer.
    -- A device_notice is written by the server, in the person's self chat only.
    -- A line is written by the server in a group, in the same step as a membership change: its
    -- body is readable JSON, its device the actor's, and it has no signature.
    kind               INTEGER CHECK (kind BETWEEN 0 AND 5),
    -- [RBAC, Chat list, Calls] what a content message is, and NULL for every other kind. Readable
    -- by the server even in an encrypted chat, so roles can allow or refuse each one:
    -- 0 text, 1 voice, 2 reaction, 3 image, 4 music (an MP3), 5 video, 6 doc, 7 file,
    -- 8 call (one per call, saved, and can be replied to; everything about it is in the call
    -- table, and this row holds nothing more), 9 media_bundle (several images and videos in one message),
    -- 10 question (a tree of questions), 11 answer (one account's answer to one), 12 sticker (names
    -- one of the server's stickers inside its encryption), 13 suggestion (what to do with its chat,
    -- such as its folder), 14 ack (one account's accepting or declining one; moves neither
    -- `last_at` nor the preview), 15 activity (one step an agent says it is taking).
    content_kind       INTEGER CHECK (content_kind BETWEEN 0 AND 15),
    -- [Unencrypted messages] the flag that tells a client how to read the body.
    encrypted          INTEGER CHECK (encrypted IN (0, 1)),
    -- [Restore, Commit order] the key this was encrypted under, and for a commit, the step it
    -- claims. NULL for an unencrypted message that is not a commit.
    epoch              INTEGER,
    -- [Topics] the topic a content message is in. NULL for General, and in every direct chat.
    topic_id           BLOB,
    -- [Threads] the seq of the main-list message whose thread this is in. Readable, so the server
    -- enforces a channel's rule, counts replies and syncs one thread.
    thread_seq         INTEGER,
    -- [Actions, Replies] the target's seq. In an encrypted chat only delete and edit fill it,
    -- because the server acts on those. Reply and reaction pointers stay inside the ciphertext.
    pointer_seq        INTEGER,
    -- [Resend check] the client's clock.
    client_sent_at     INTEGER,
    -- [Chat list, Retention, Sync] the server's clock, which never goes back. A removed message keeps
    -- the time it was removed instead, so sync hands it out and every device drops its copy.
    server_saved_at    INTEGER NOT NULL,
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
    CHECK (kind <> 3 OR epoch IS NOT NULL),
    CHECK (kind NOT IN (1, 2) OR pointer_seq IS NOT NULL),
    CHECK (content_kind IS NOT 2 OR encrypted = 1 OR pointer_seq IS NOT NULL),
    CHECK (kind <> 4 OR encrypted = 0),
    CHECK (kind <> 4 OR expires_at IS NULL),
    -- A row is either a whole message or only its seq, never partly wiped.
    CHECK ((kind IS NULL) = (device_id IS NULL)),
    CHECK ((kind IS NULL) = (encrypted IS NULL)),
    CHECK ((kind IS NULL) = (client_sent_at IS NULL)),
    -- A line is the server's own and carries no signature.
    CHECK ((kind IS NULL OR kind = 5) = (signature IS NULL)),
    CHECK ((kind IS NULL) = (body IS NULL)),
    CHECK (kind = 0 OR (topic_id IS NULL AND thread_seq IS NULL)),
    CHECK (kind IS NOT NULL OR (epoch IS NULL AND pointer_seq IS NULL AND expires_at IS NULL))
) STRICT;

-- [Chat list] finds the preview after the last message is removed.
CREATE INDEX message_preview ON message (conversation_id, seq) WHERE kind = 0;
-- [Commit order] the first commit for an epoch wins, and a second cannot be stored.
CREATE UNIQUE INDEX message_commit ON message (conversation_id, epoch) WHERE kind = 3;
-- [Delete] an edit holds a message's newer text, so deleting the message must find and remove
-- its edits too. Edit pointers are readable, which is what makes this possible.
CREATE INDEX message_edit ON message (conversation_id, pointer_seq) WHERE kind = 1;
-- [Calls] the calls, to end those still open at startup, since nobody can be in them after a restart.
CREATE INDEX message_call ON message (conversation_id, seq) WHERE content_kind = 8;
-- [Topics] removing a topic wipes its messages.
CREATE INDEX message_topic ON message (conversation_id, topic_id) WHERE topic_id IS NOT NULL;
-- [Threads] a thread's replies, counted under the message that opened it.
CREATE INDEX message_thread ON message (conversation_id, thread_seq) WHERE thread_seq IS NOT NULL;
-- [Sync] what was saved or removed after a time, in one chat or several.
CREATE INDEX message_sync ON message (conversation_id, server_saved_at);
-- [Retention] the sweep.
CREATE INDEX message_expiry ON message (expires_at) WHERE expires_at IS NOT NULL;

-- [Calls, Sync] everything about a call, many rows per call and never chat rows: joins, leaves and
-- the end written by the server, and declines and text from clients. The call's message holds nothing
-- more. A call with no end row is still open.
CREATE TABLE call (
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
    -- [Calls, Sync] the server's clock, which never goes back. An end's is when the call ended.
    server_at          INTEGER NOT NULL,
    -- [Calls, Signatures] text only: the client's clock, the key it is sealed under, its signature
    -- over the header and body, and the ciphertext. The header names call_seq, so it can never
    -- pass for a chat message.
    client_at          INTEGER,
    epoch              INTEGER,
    signature          BLOB,
    body               BLOB,
    -- [Calls, Chat list] an end's only: 0 missed, 1 answered. Readable so the list can show which.
    -- A call does not count as unread.
    outcome            INTEGER CHECK (outcome IN (0, 1)),
    PRIMARY KEY (conversation_id, call_seq, n),
    FOREIGN KEY (conversation_id, call_seq) REFERENCES message (conversation_id, seq)
        ON DELETE CASCADE,
    CHECK ((kind = 3) = (body IS NOT NULL)),
    CHECK ((kind = 3) = (signature IS NOT NULL)),
    CHECK ((kind = 3) = (epoch IS NOT NULL)),
    CHECK ((kind = 3) = (client_at IS NOT NULL)),
    CHECK ((kind = 4) = (outcome IS NOT NULL))
) WITHOUT ROWID, STRICT;

-- [Sync] call rows saved after a time, in one chat or several.
CREATE INDEX call_sync ON call (conversation_id, server_at);

-- [Chat history, Membership lines] what happened in a group or channel, written by the server so a
-- modified client cannot leave one out: adds, removals, leaving, role changes, deletes of others'
-- messages, topic changes and a declined invitation. Kept under the chat's retention.
CREATE TABLE chat_event (
    -- [Chat history] which chat.
    conversation_id  BLOB NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
    -- [Chat history] 1, 2, 3 in the chat, in the order the server wrote them.
    n                INTEGER NOT NULL CHECK (n >= 1),
    -- [Chat history] who did it.
    actor            BLOB NOT NULL REFERENCES account (id),
    -- [Chat history] 0 created, 1 added, 2 removed, 3 left, 4 let in, 5 joined by invitation, 6 made
    -- owner, 7 made moderator, 8 made member, 9 deleted a message, 10 topic added, 11 topic removed,
    -- 12 topics on, 13 topics off, 14 invitation declined, 16 a topic's profile edited, 17 a topic's
    -- options changed, 18 made a topic's moderator, 19 made a member there again, 20 edited a message.
    action           INTEGER NOT NULL CHECK (action BETWEEN 0 AND 20 AND action != 15),
    -- [Chat history] the account acted on: the member, or a deleted or edited message's author.
    target           BLOB,
    -- [Chat history] the message a delete removed or an edit changed.
    seq              INTEGER,
    -- [Topic settings] the topic whose history the row is in, NULL for the group's own.
    topic_id         BLOB,
    -- [Chat history] a delete's or edit's copy of the message and its edits as they were, as the
    -- JSON list of message frames the wire carries: ciphertext, header and signature, which the
    -- history's readers open and check. NULL for every other row.
    copy             BLOB,
    -- [Chat history] who sees it: 1 owners and moderators, 2 the
    -- account in `viewer` alone.
    audience         INTEGER NOT NULL CHECK (audience IN (1, 2)),
    -- [Invitations] the one account an audience 2 row is for.
    viewer           BLOB REFERENCES account (id),
    -- [Chat history, Sync] the server's clock, which never goes back.
    server_at        INTEGER NOT NULL,
    PRIMARY KEY (conversation_id, n),
    CHECK ((audience = 2) = (viewer IS NOT NULL))
) WITHOUT ROWID, STRICT;

-- [Sync] chat events saved after a time, in one chat or several.
CREATE INDEX chat_event_sync ON chat_event (conversation_id, server_at);


-- [Seen times] one row per `seen`, with one time for many messages. A member saw message N
-- at the time on their first row reaching N. Deleted once the messages it covers expire.
-- Not tied to `member`: a member who leaves keeps their seen times, so the code checks
-- membership before writing one.
CREATE TABLE seen (
    -- [Seen times] which chat.
    conversation_id  BLOB NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
    -- [Seen times] who.
    account_id       BLOB NOT NULL REFERENCES account (id),
    -- [Topics, Threads] where it was read: 0 the main list or General, 1 a topic, 2 a thread.
    kind             INTEGER NOT NULL DEFAULT 0 CHECK (kind IN (0, 1, 2)),
    -- [Topics, Threads] empty for the main list, the topic's id, or the thread's seq as 8 bytes
    -- big-endian. A place nobody read in has no rows.
    place            BLOB NOT NULL DEFAULT x'',
    -- [Seen times, Read state] the "seen up to N" the client sent.
    up_to_seq        INTEGER NOT NULL,
    -- [Devices] which device reported it.
    device_id        BLOB NOT NULL REFERENCES device (id),
    -- [Seen times] accurate for reading offline, shown unless later than the server's or far
    -- from it.
    client_seen_at   INTEGER NOT NULL,
    -- [Seen times] catches a client lying about its time.
    server_seen_at   INTEGER NOT NULL,
    PRIMARY KEY (conversation_id, account_id, kind, place, up_to_seq)
) WITHOUT ROWID, STRICT;

-- [Received times] one row per `received`, like `seen`: a member's device got message N at the time
-- on their first row reaching N. Deleted once the messages it covers expire. Not tied to `member`,
-- so a member who leaves keeps their received times.
CREATE TABLE received (
    -- [Received times] which chat.
    conversation_id     BLOB NOT NULL REFERENCES conversation (id) ON DELETE CASCADE,
    -- [Received times] who.
    account_id          BLOB NOT NULL REFERENCES account (id),
    -- [Topics, Threads] as `seen`'s: 0 the main list, 1 a topic, 2 a thread.
    kind                INTEGER NOT NULL DEFAULT 0 CHECK (kind IN (0, 1, 2)),
    -- [Topics, Threads] as `seen`'s.
    place               BLOB NOT NULL DEFAULT x'',
    -- [Received times, Read state] the "received up to N" the client sent.
    up_to_seq           INTEGER NOT NULL,
    -- [Devices] which device reported it.
    device_id           BLOB NOT NULL REFERENCES device (id),
    -- [Received times] the device's own time, shown unless later than the server's or far from it.
    client_received_at  INTEGER NOT NULL,
    -- [Received times] catches a client lying about its time.
    server_received_at  INTEGER NOT NULL,
    PRIMARY KEY (conversation_id, account_id, kind, place, up_to_seq)
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
    -- [Profile images] the account it belongs to, its bytes under `accounts/<account id>`. NULL for a
    -- chat's file. A photo is one chunk, of kind 3, and used_at is set once the profile names it.
    account_id       BLOB REFERENCES account (id) ON DELETE CASCADE,
    -- [Unencrypted messages] 0 only for an unencrypted message's files, as its uploader declared.
    encrypted        INTEGER NOT NULL CHECK (encrypted IN (0, 1)),
    -- [Upload limits] the type its uploader declared, which picks the limit: 1 voice, 3 image,
    -- 4 music, 5 video, 6 doc, 7 file, as message.content_kind. The real type is encrypted.
    kind             INTEGER NOT NULL CHECK (kind IN (1, 3, 4, 5, 6, 7)),
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

-- [Stickers] a named pack of the server's stickers, which everyone with a registered device can list
-- and send. Added and removed with `sticker.manage`.
CREATE TABLE sticker_pack (
    -- [Stickers] what the routes and the command line name a pack by.
    id               BLOB PRIMARY KEY,
    -- [Stickers] unique, so two packs never look the same in a picker. 1 to 64 characters.
    name             TEXT NOT NULL UNIQUE,
    -- [Stickers] packs are listed oldest first.
    created_at       INTEGER NOT NULL
) WITHOUT ROWID, STRICT;

-- [Stickers] one sticker: a static or animated WebP, not encrypted, whose bytes are under
-- `[files] directory` in `stickers`, a file named by its id. An id is never reused, so its bytes never
-- change. A sticker message names it inside its encryption, so nothing here says who sent which.
CREATE TABLE sticker (
    -- [Stickers] names the file on disk, and what a sticker message holds.
    id               BLOB PRIMARY KEY,
    -- [Stickers] removing a pack removes its stickers.
    pack_id          BLOB NOT NULL REFERENCES sticker_pack (id) ON DELETE CASCADE,
    -- [Stickers] one optional emoji, for search later.
    emoji            TEXT,
    -- [Stickers] read from its WebP header, at most 512 each, so a client draws its space first.
    width            INTEGER NOT NULL CHECK (width BETWEEN 1 AND 512),
    -- [Stickers] the same for its height.
    height           INTEGER NOT NULL CHECK (height BETWEEN 1 AND 512),
    -- [Stickers, Upload limits] its bytes, at most `[limits.upload] sticker`.
    size             INTEGER NOT NULL CHECK (size > 0),
    -- [Stickers] a pack's stickers are listed oldest first.
    created_at       INTEGER NOT NULL
) WITHOUT ROWID, STRICT;

-- [Stickers] a pack's stickers, in order.
CREATE INDEX sticker_in_pack ON sticker (pack_id, created_at);


-- ---------------------------------------------------------------------------
-- MLS and keys
-- ---------------------------------------------------------------------------

-- [External join, Commit order] every chat has one. Kept off
-- `conversation` because group info can be large and that row changes on every message.
CREATE TABLE mls_group (
    -- [External join] which chat.
    conversation_id  BLOB PRIMARY KEY REFERENCES conversation (id) ON DELETE CASCADE,
    -- [Commit order] a commit must be for this epoch, or it lost the race and must retry.
    epoch            INTEGER NOT NULL,
    -- [External join] public, and what a new device joins the chat with by itself.
    group_info       BLOB NOT NULL,
    -- [Commit check] openmls's public group, started from epoch 0's group info and moved on by
    -- each accepted commit, which the server reads to check what a commit adds and removes.
    public_group     BLOB NOT NULL,
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

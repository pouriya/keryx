# keryx REST API

Everything here is under `{origin}/api/v1`. Every reply is wrapped:

    {"ok": true, "data": {...}}
    {"ok": false, "code": "...", "message": "..."}

Success is 2xx and failure is 4xx or 5xx. `code` is for programs and `message` for people. Binary
fields are standard base64 with padding, and ids are ULID text. Secrets go in bodies and headers,
never in a URL.

## Proving who you are

A browser logs in with `POST /login`, which sets an `HttpOnly` cookie named `keryx_session`. Anything
else logs in with `POST /login/token`, which returns a token, and sends it as
`Authorization: Bearer <token>`.

## Anyone

    GET  /api/v1                 this guide
    GET  /public/features        -> registration, captcha
    GET  /captcha                -> id, image. Only with a [captcha] section
    POST /register               username, password, display_name?, captcha?{id, answer} -> account
    POST /login                  username, password, captcha?, device?{id, challenge, signature}
                                 -> account, device?
    POST /login/token            the same -> account, device?, token
    POST /setup                  token, password -> account, username
    POST /challenge              username, purpose -> challenge
    POST /password/reset         username, challenge, new_password, signature

`purpose` is `password_reset` or `device_login`. A challenge lasts 5 minutes and works once. A
returning device signs one with its own key and gives it at login, and otherwise a login has no
device. A reset is signed by the master key, and ends all of the account's sessions.

Registering and the one-time link do not log you in.

## Any session

    GET  /sessions               -> session_list
    DEL  /sessions/{id}
    PUT  /account/keys           master_public, account_public, account_stamp -> self_chat
    GET  /accounts/{id}          -> id, username, display_name?, master_public?, account_public?,
                                    account_stamp?
    GET  /devices                -> device_list
    POST /devices/id             -> device
    POST /devices                device, public_key, stamp, platform, os, os_version,
                                 web_platform?, web_platform_version? -> device
    POST /devices/{id}/terminate signature
    POST /devices/{id}/logout    signature

`PUT /account/keys` works once. A session with no device gets a reduced `device_list`: `id`,
`platform` and `last_active_day`, which is days since the Unix epoch.

To add a device: ask `POST /devices/id` for its id, which lasts 5 minutes and works once, stamp that id
with the master key, then `POST /devices`. `platform` is 0 for desktop and 1 for mobile. The session
then belongs to that device, and the device's notice appears in the account's self chat.

`terminate` is signed by the master key, and `logout` by the device's own key. Both end the device for
good.

## With a named permission

    POST /accounts               username, admin?, display_name? -> account, link
    GET  /accounts?search=       -> account_list

## With a registered device, under the account's device limit

    POST /key_packages                    body_list -> count
    POST /accounts/{id}/key_packages/claim   -> key_package_list{device, body}
    POST /chats                           account -> chat, created
    GET  /chats/{id}/group_info           -> epoch, group_info
    GET  /chats/{id}/devices              -> device_list{account, device, public_key, stamp,
                                             master_public, statement?}
    GET  /chats/{id}/wraps                -> wrap_list{epoch, account, wrapped}
    POST /chats/{id}/files                kind, size, chunks -> file
    PUT  /files/{id}/chunks/{n}           the encrypted chunk's raw bytes -> size, chunks, received
    GET  /files/{id}                      -> size, chunks, received
    GET  /files/{id}/chunks/{n}           -> the encrypted chunk's raw bytes
    GET  /chats/{id}/calls/{seq}/events   -> event_list, what happened inside a call

Claiming hands out one package per device and deletes it. `POST /chats` gives the pair's chat, and your
own account id gives your self chat. The `/chats/{id}` routes need you to be in that chat. In
`device_list`, a device with a `statement` has ended, and that is its signed revocation or logout.

A file is encrypted by its sender in chunks of 1 MiB, each followed by its 16-byte tag. `kind` is the
type that picks the upload limit (1 voice, 3 image, 4 video, 5 doc, 6 file), and `size` counts the
encrypted bytes. Chunks go up in order, and sending the last one again is fine, so after a drop, ask
`GET /files/{id}` and go on from `received`. Only the uploader's account puts chunks. A `send` keeps
its files by listing them in `persist`, and until then only the uploader reads them. An edit or delete
lists the files it drops in `remove`. An upload no sent message keeps is removed after a day.

## The WebSocket

    GET /ws

Send the cookie or the Bearer header, and offer the `keryx.v1` subprotocol. A refused connection closes
with code 4403 and the refusal's code as the reason. A server shutting down closes with 1001.

The first frame is always `features`, with the server's `ping_after`, `ping_every` and `ping_timeout` in
seconds. After `ping_after` with nothing from you, the server sends a WebSocket ping, then one every
`ping_every`, and you have `ping_timeout` to answer each or the connection closes. A browser answers by
itself. To check the server yourself, send a WebSocket ping, or a `ping` frame, which gets `done`.

Every frame is JSON with a `type`. A request's `ref` is a ULID you make, which comes back in its answer.

You send: `send`, `seen`, `received`, `typing`, `sync`, `history`, `group_info`, `wraps`, `call_event`,
`ping`.

The server sends: `features`, `sent`, `done`, `refused`, `message`, `chats`, `page`, `welcome`, `typing`,
`marks`, `revoked`, `call_event`.

`received` and `seen` each carry `chat` and `up_to_seq`, and the server keeps the highest of each. When
one rises, the other members' online devices get `marks` with the `account`, its `received_seq` and its
`seen_seq`. Each `chats` entry carries the same positions in `mark_list`, and its newest content
message in `last_content`. Neither goes out in a channel.

`history` returns up to `limit` messages after `after_seq`, oldest first. With `before_seq`, it
returns the newest `limit` below it instead, still oldest first.

A commit is a `send` with kind 3, carrying `epoch`, `wrap_list`, `welcome_list` and `group_info`, the
chat's group info at that epoch, which is saved with it. The first commit for each epoch wins, and a
loser gets `refused` with the code `commit_lost`. A chat's group starts when someone sends `group_info`
for epoch 0, and if two people start it at once the second gets `chat_started` and waits for its
Welcome. The same epoch 0 bytes sent again are accepted, as a resend whose answer was lost.

## Calls

A call is one `send` with content kind 7, in a direct chat only, and its delivery is the ring. A chat
has one open call at a time: starting another is refused with `call_open`, and the refusal carries the
open call's `call_seq`, so join that one instead. Calls elsewhere are refused with `call_not_allowed`.

The other account declines with a `call_event` frame of `kind` 2 and the call's `call_seq`, answered
with `done`, and in a direct chat that ends the call as missed. A call nobody answers ends as missed
after 60 seconds. Every `join`, `leave`, `decline` and `end` reaches the chat's online devices as
`call_event`: `{chat, call_seq, n, kind, device, server_at}`, with `kind` 0 join, 1 leave, 2 decline,
3 text and 4 end. The call's message then carries `call_outcome`, 0 missed or 1 answered, and
`call_ended_at`. An ended call cannot be joined again.

## Common refusal codes

`not_logged_in`, `no_permission`, `no_device`, `device_limit`, `not_member`, `bad_body`,
`body_too_large`, `captcha_wrong`, `bad_login`, `bad_signature`, `unknown_device_id`.

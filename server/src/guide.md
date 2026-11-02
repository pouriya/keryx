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

Claiming hands out one package per device and deletes it. `POST /chats` gives the pair's chat, and your
own account id gives your self chat. The `/chats/{id}` routes need you to be in that chat. In
`device_list`, a device with a `statement` has ended, and that is its signed revocation or logout.

## The WebSocket

    GET /ws

Send the cookie or the Bearer header, and offer the `keryx.v1` subprotocol. A refused connection closes
with code 4403 and the refusal's code as the reason. A server shutting down closes with 1001.

The first frame is always `features`, with the server's `ping_after`, `ping_every` and `ping_timeout` in
seconds. After `ping_after` with nothing from you, the server sends a WebSocket ping, then one every
`ping_every`, and you have `ping_timeout` to answer each or the connection closes. A browser answers by
itself. To check the server yourself, send a WebSocket ping, or a `ping` frame, which gets `done`.

Every frame is JSON with a `type`. A request's `ref` is a ULID you make, which comes back in its answer.

You send: `send`, `seen`, `received`, `typing`, `sync`, `history`, `group_info`, `wraps`, `ping`.

The server sends: `features`, `sent`, `done`, `refused`, `message`, `chats`, `page`, `welcome`, `typing`,
`marks`, `revoked`.

`received` and `seen` each carry `chat` and `up_to_seq`, and the server keeps the highest of each. When
one rises, the other members' online devices get `marks` with the `account`, its `received_seq` and its
`seen_seq`. Each `chats` entry carries the same positions in `mark_list`, and its newest content
message in `last_content`. Neither goes out in a channel.

`history` returns up to `limit` messages after `after_seq`, oldest first. With `before_seq`, it
returns the newest `limit` below it instead, still oldest first.

A commit is a `send` with kind 3, carrying `epoch`, `wrap_list` and `welcome_list`. The first commit for
each epoch wins, and a loser gets `refused` with the code `commit_lost`. A chat's group starts when
someone sends `group_info` for epoch 0, and if two people start it at once the second gets
`chat_started` and waits for its Welcome.

## Common refusal codes

`not_logged_in`, `no_permission`, `no_device`, `device_limit`, `not_member`, `bad_body`,
`body_too_large`, `captcha_wrong`, `bad_login`, `bad_signature`, `unknown_device_id`.

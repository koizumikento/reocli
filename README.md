# reocli

Rust ベースの Reolink CLI/MCP 実験用プロジェクトです。

## Structure

- `src/core`: ドメイン共通型/エラー/コマンド定義
- `src/reolink`: Reolink API 呼び出し層
- `src/app`: CLI/MCP 共通のユースケース
- `src/interfaces`: CLI と MCP の入出力アダプタ
- `src/bin`: 実行エントリ (`reocli`, `reocli-mcp`)

## Configuration (Environment Variables)

- `REOCLI_ENDPOINT`: API エンドポイント。未設定時は `https://camera.local`。
- `REOCLI_TOKEN`: トークン認証に使用。
- `REOCLI_TOKEN_CACHE_PATH`: トークンキャッシュファイルパス。未設定時は `~/.reocli/tokens/<endpoint>.token`。
- `REOCLI_USER`: ユーザー名。`REOCLI_PASSWORD` が設定されていて未設定/空文字の場合は `admin` を使用。
- `REOCLI_PASSWORD`: パスワード認証に使用。
- `REOCLI_PTZ_BACKEND`: PTZ制御バックエンド。`cgi`（既定）または `onvif`。
- `REOCLI_ONVIF_DEVICE_SERVICE_URL`: ONVIF Device Service URL（例: `http://<camera-ip>:8000/onvif/device_service`）。
- `REOCLI_ONVIF_PROFILE_TOKEN`: ONVIF ProfileToken の明示指定（未指定時は `GetProfiles` から自動解決）。
- `REOCLI_ONVIF_PORT`: `REOCLI_ONVIF_DEVICE_SERVICE_URL` 未設定時の既定ポート（既定: `8000`）。

認証の優先順:

1. `REOCLI_TOKEN`
2. `REOCLI_TOKEN_CACHE_PATH`（または既定パス）のキャッシュトークン
3. `REOCLI_USER` + `REOCLI_PASSWORD`（`REOCLI_USER` が空なら `admin`）
4. 匿名認証

`REOCLI_PASSWORD` がある状態でログインが発生すると、取得したトークンはキャッシュファイルへ更新されます。認証失敗で再ログインが必要になった場合は古いキャッシュを削除してから更新します。

## Implemented CLI Commands

- `reocli get-user-auth <user> <password>`
- `reocli get-ability [user]`
- `reocli get-dev-info`
- `reocli get-channel-status [channel]`
- `reocli get-ptz-status [channel]`
- `reocli get-time`
- `reocli get-net-port`
- `reocli set-time <iso8601>`
- `reocli set-onvif <on|off> [--port <1-65535>]`
- `reocli snap [channel] [--out path]`
- `reocli ptz move <direction> [--speed <1-64>] [--duration <ms>] [--channel <0-255>]`
- `reocli ptz stop [--channel <0-255>]`
- `reocli ptz preset list [--channel <0-255>]`
- `reocli ptz preset goto <preset_id> [--channel <0-255>]`
- `reocli ptz calibrate auto [--channel <0-255>]`
- `reocli ptz set-absolute <pan_count> <tilt_count> [--tol-count <i64>] [--timeout-ms <u64>] [--channel <0-255>]`
- `reocli ptz get-absolute [--channel <0-255>]`
- `reocli preflight [user]`

`snap` と PTZ 制御系コマンド（`move` / `stop` / `preset goto` / `calibrate auto` / `set-absolute`）は実行前に `GetAbility` でサポート確認し、未対応なら `UnsupportedCommand` で失敗します。

## Implemented MCP Tools

- `mcp.list_tools`
- `reolink.get_user_auth`
- `reolink.get_ability`
- `reolink.get_dev_info`
- `reolink.get_channel_status`
- `reolink.get_ptz_status`
- `reolink.get_time`
- `reolink.set_time`
- `reolink.get_net_port`
- `reolink.set_onvif_enabled`
- `reolink.snap`
- `reolink.ptz_move`
- `reolink.ptz_stop`
- `reolink.ptz_preset_list`
- `reolink.ptz_preset_goto`
- `reolink.ptz_calibrate_auto`
- `reolink.ptz_set_absolute`
- `reolink.ptz_get_absolute`

## Examples

```bash
# Auth
reocli get-user-auth admin secret

# Device / time
reocli get-dev-info
reocli get-time
reocli get-net-port
reocli set-time 2026-02-25T10:00:00Z
reocli set-onvif on --port 8000

# Snap
reocli snap 0 --out snapshots/front-door.jpg

# PTZ
reocli ptz move left --speed 6 --duration 300 --channel 0
reocli ptz stop --channel 0
reocli ptz preset list --channel 0
reocli ptz preset goto 7 --channel 0
reocli ptz calibrate auto --channel 0
reocli ptz set-absolute 1500 -180 --tol-count 12 --timeout-ms 25000 --channel 0
reocli ptz get-absolute --channel 0

# PTZ (ONVIF ContinuousMove backend)
REOCLI_PTZ_BACKEND=onvif \
REOCLI_ENDPOINT=https://192.168.0.220 \
REOCLI_USER=admin \
REOCLI_PASSWORD='******' \
reocli ptz set-absolute 1500 -180 --tol-count 12 --timeout-ms 25000 --channel 0
```

```bash
# MCP
reocli-mcp reolink.get_net_port
reocli-mcp reolink.set_onvif_enabled on 8000
reocli-mcp reolink.ptz_calibrate_auto 0
reocli-mcp reolink.ptz_set_absolute 0 1500 -180 12 25000
reocli-mcp reolink.ptz_get_absolute 0
```

## Testing

通常テスト（mockito ベース）:

```bash
cargo test --all-targets --all-features
```

実機テスト（`REOCLI_LIVE_TEST=1` のときのみ実行）:

```bash
REOCLI_LIVE_TEST=1 \
REOCLI_ENDPOINT=https://<camera-host> \
REOCLI_TOKEN=<token> \
cargo test --test live_smoke -- --nocapture
```

## PTZ Absolute (Raw Count)

- `set-absolute` / `get-absolute` は角度ではなく `GetPtzCurPos` の生カウント値で扱います。
- `set-absolute` は `pan_count` / `tilt_count` を目標として、`tol_count` 以内になるまで PTZ 制御を繰り返します。

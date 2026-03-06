# Reolink 公式 CGI 利用メモ

このドキュメントは、Reolink 公式サポート記事に書かれている CGI 利用手順を、最初に動かすための最小セットとして整理したものです。

## このドキュメントのスコープ

- ここでは raw な CGI endpoint と、その疎通確認に使う最小コマンドだけを扱う。
- `reocli` / `reocli-mcp` の公開コマンド一覧と環境変数は `README.md` を参照する。
- ONVIF backend や PTZ absolute 制御の詳細は `docs/ptz-absolute-control-spec.md` を参照する。

## 1. まず確認すること

- 対応機種かどうかを確認する。
  - 公式一覧: `Which Reolink Products Support CGI_RTSP_ONVIF?`
- CGI/API が使える状態かを確認する。
  - 公式トラブルシュートでは、未対応機種・無効化状態・認証ミス・JSON 形式ミスが失敗要因として挙げられている。
- ネットワーク到達性とポートを確認する。
  - 公式記事では `HTTPS=443` / `HTTP=80` が案内されている。

## 2. CGI の基本 URL 形式

公式記事（`How to send CGI Commands and draw up a CGI User Manual...`）では、次の 2 形式が案内されています。

- token 認証
  - `https://<IP>/cgi-bin/api.cgi?cmd=<Command>&token=<Token>`
- ユーザー/パスワード認証
  - `https://<IP>/cgi-bin/api.cgi?cmd=<Command>&user=<User>&password=<Password>`

補足:

- 公式記事では安全性の観点から `HTTPS` 利用を推奨。
- 実運用では URL に平文資格情報を含めないため、可能な限り token 形式を優先する。

## 3. 最小実行フロー（公式例ベース）

### 3-1. `GetAbility` で利用可能コマンドを確認

機種・ファームウェア差分があるため、最初に `GetAbility` を実行して使える機能を確認する。

```bash
curl -k -X POST "https://<IP>/cgi-bin/api.cgi?cmd=GetAbility&user=<USER>&password=<PASSWORD>" \
  -H 'content-type: application/json' \
  -d '[{"cmd":"GetAbility","action":0,"param":{"User":{"userName":"<USER>"}}}]'
```

### 3-2. `GetDevInfo` で API 通信確認

```bash
curl -k -X POST "https://<IP>/cgi-bin/api.cgi?cmd=GetDevInfo&user=<USER>&password=<PASSWORD>" \
  -H 'content-type: application/json' \
  -d '[{"cmd":"GetDevInfo","action":0,"param":{}}]'
```

### 3-3. `Snap` で静止画取得（ブラウザ/HTTP GET）

公式 `Snap` 記事の URL 例:

```text
https://<USER>:<PASSWORD>@<IP>/cgi-bin/api.cgi?cmd=Snap&channel=0&rs=flsYJfZgM6RTB_os&user=<USER>&password=<PASSWORD>
```

補足:

- 証明書エラーで `HTTPS` が通らない場合は、公式記事でも `HTTP` への切り替えが案内されている。
- `channel` はカメラ/NVR 構成に応じて変更する。

## 4. エラー時の切り分け（公式記事ベース）

`What Should I Do if Reolink API Command Returns an Error?` で挙がっている代表例:

- `401 Unauthorized`
  - ユーザー名/パスワード不一致、URL 記法ミスを確認。
- `400 Bad Request`
  - JSON 形式ミス、未対応コマンド、対象機種で CGI が無効などを確認。

最初の切り分け手順:

1. URL（IP/ポート/`cmd`）が正しいか確認
2. 認証情報が正しいか確認
3. `GetAbility` の結果で対象 `cmd` がサポートされているか確認
4. JSON ボディを最小構成にして再実行

## 5. 公式参照

- [How to send CGI Commands and draw up a CGI User Manual for Reolink devices](https://support.reolink.com/hc/en-us/articles/360018746874-How-to-send-CGI-Commands-and-draw-up-a-CGI-User-Manual-for-Reolink-devices/)
- [How to Capture Live Viewing Image of Reolink Cameras in Web Browsers](https://support.reolink.com/hc/en-us/articles/360003424893-How-to-Capture-Live-Viewing-Image-of-Reolink-Cameras-in-Web-Browsers/)
- [What Should I Do if Reolink API Command Returns an Error?](https://support.reolink.com/hc/en-us/articles/360037571173-What-Should-I-Do-if-Reolink-API-Command-Returns-an-Error/)
- [Which Reolink Products Support CGI_RTSP_ONVIF?](https://support.reolink.com/hc/en-us/articles/900000625446-Which-Reolink-Products-Support-CGI-RTSP-ONVIF/)

---

## 6. `reocli` が利用している CGI コマンド

`reocli` の公開コマンドは raw CGI と 1 対 1 ではありません。内部では主に次の CGI を組み合わせています。

- 認証/能力確認
  - `GetUserAuth`
  - `GetAbility`
  - `Login`
- デバイス/ネットワーク/時刻
  - `GetDevInfo`
  - `GetChannelStatus`
  - `GetTime`
  - `SetTime`
  - `GetNetPort`
  - `SetNetPort`
- スナップショット
  - `Snap`
- PTZ 状態/制御
  - `GetPtzCurPos`
  - `GetZoomFocus`
  - `GetPtzPreset`
  - `GetPtzCheckState`
  - `PtzCtrl`

対応関係の目安:

- `reocli preflight [user]` は `GetAbility` と `GetDevInfo` をまとめて実行する。
- `reocli set-onvif <on|off> [--port ...]` は `GetNetPort` / `SetNetPort` を使う。
- `reocli ptz move` / `stop` / `preset goto` は主に `PtzCtrl` を使う。
- `reocli ptz set-absolute` は高レベルの閉ループ制御で、内部的に CGI と ONVIF の両方を使い分けることがある。

## 7. 既知の機種差分メモ

- `GetAbility` の結果は機種・FWで差があるため、`snap` と PTZ 実行前に対応可否をガードしている。
- `GetChannelStatus` は機種によりレスポンスキー名差分（`channel/channelId/channelNo`, `online/status/state`）があるため、実装側で吸収している。
- ONVIF backend を使う補助コマンド（`reocli ptz onvif ...`）は CGI の 1 対 1 ラッパーではなく、CLI 専用の診断/制御入口として別実装になっている。

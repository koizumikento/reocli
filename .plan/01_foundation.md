# 01 Foundation: HTTP/CGI Base

## Goal
`reqwest + serde` の実HTTP基盤を、全機能で再利用できる状態にする。

## Owned Files
- `src/reolink/client.rs`
- `src/core/error.rs`
- (必要なら) `src/core/model.rs`
- (必要なら) `tests/client_http.rs`

## Tasks
1. `client.execute()` の責務を固定
- URL: `<endpoint>/cgi-bin/api.cgi`
- Query: `cmd` + (`token` or `user/password`)
- Body: `[{"cmd":...,"action":0,"param":...}]`

2. 共通レスポンス正規化
- JSON配列レスポンスを文字列で返すだけでなく、後段がパースしやすい形に統一。
- `code != 0` を `AppError` にマップする補助関数を提供。

3. エラー分類を統一
- `401` は `Authentication`
- 通信失敗/非2xx は `Network`
- JSON不正は `UnexpectedResponse`

4. タイムアウト/証明書方針
- 既定タイムアウトを設定。
- 自己署名証明書向け `allow_insecure_tls` はフラグで明示制御。

## Done Criteria
- `cargo check/fmt/clippy/test` が通る。
- モックHTTPで成功/失敗/401/JSON不正を単体テストできる。
- 後続ワークストリームが `client.execute()` のみで実装可能。

## Risks
- 機種により `param` 形式が微妙に違う。
- 先に厳密化しすぎると後続機能が詰まるため、最小限の厳密化に留める。

# 07 Test / CI / Docs

## Goal
並列実装後の品質を統合し、壊れにくい運用にする。

## Owned Files
- `tests/*.rs` (統合)
- `README.md`
- `docs/reolink-cgi.md` (実装済みコマンド一覧追記)
- (必要なら) `.github/workflows/ci.yml`

## Tasks
1. テスト層の分離
- `mockito` ベースの高速テスト。
- 実機テストは `REOCLI_LIVE_TEST=1` でのみ実行。

2. 回帰ケース
- 認証失敗 (`please login first`, `401`)
- 未対応コマンド (`GetAbility` で拒否)
- PTZ引数不正（speed/direction）

3. CLI/MCPの契約確認
- CLI表示フォーマットを固定。
- MCPレスポンスJSONフォーマットを固定。

4. ドキュメント
- 環境変数例: `REOCLI_ENDPOINT`, 認証情報、ライブテスト条件。
- 実装済みコマンド一覧と既知の機種差分を記載。

## Done Criteria
- `cargo check --all-targets --all-features`
- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-targets --all-features`
- READMEのコマンド例で実行できる。

## Dependencies
- 02〜06 完了後

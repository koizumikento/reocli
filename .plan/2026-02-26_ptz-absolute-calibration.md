# PTZ Absolute Control + Auto Calibration Plan

- Date: `2026-02-26`
- Status: `draft`
- Related spec: `docs/ptz-absolute-control-spec.md`

## Goal

`Pan/Tilt` を対象に、以下を実装する。

1. 自動キャリブレーション（外部センサなし）
2. 境地基準での絶対位置指定
3. 状態方程式ベースの閉ループ制御

## Out of Scope

- `Zoom` の絶対制御
- 真北など外部基準に拘束された絶対方位制御
- 新規ハードウェアセンサ追加

## Owned Files

- `src/core/model.rs`
- `src/app/usecases/mod.rs`
- `src/app/usecases/ptz_calibrate_auto.rs` (new)
- `src/app/usecases/ptz_set_absolute.rs` (new)
- `src/app/usecases/ptz_get_absolute.rs` (new)
- `src/app/usecases/ptz_controller.rs` (new)
- `src/reolink/ptz.rs`
- `src/interfaces/runtime.rs`
- `src/interfaces/cli/args.rs`
- `src/interfaces/cli/handlers.rs`
- `src/interfaces/mcp/tools.rs`
- `src/interfaces/mcp/handlers.rs`
- `tests/ptz_absolute_integration.rs` (new)
- `tests/ptz_calibration_integration.rs` (new)
- `tests/cli_smoke.rs`
- `tests/mcp_smoke.rs`
- `README.md`
- `docs/ptz-absolute-control-spec.md`

## Workstreams

### 1. Domain/Model

1. `PtzStatus` と分離した制御向けモデルを追加
- `AbsolutePose`（pan_deg/tilt_deg）
- `CalibrationParams`（offset/scale/deadband/model/firmware）
- `CalibrationReport`（結果統計）
2. 状態方程式で利用する軸状態型を追加
- `AxisState`, `AxisEstimate`, `AxisModelParams`

### 2. Controller Core

1. `ptz_controller.rs` を追加
- 状態更新: `x[k+1] = A x[k] + B u[k]`
- 観測更新: `y[k] = C x[k]`
- 制御出力: 誤差 + 速度推定から `u` を生成
2. 量子化ロジックを追加
- `u in [-1, 1]` を `speed 1..64` + 方向へ変換
3. セーフティ制約
- レンジ外目標のクリップ
- タイムアウト時 `Stop` 強制送出

### 3. Auto Calibration

1. `ptz_calibrate_auto` ユースケースを追加
- 可動域走査（低速）
- 一次写像（offset/scale）推定
- 反転走査で deadband 推定
2. 保存・復元を追加
- キー: `serial + model + firmware`
- フォーマット: JSON
3. 再利用条件
- `calibration_state != 2` なら再校正
- 保存済みで適合なら再利用

### 4. Absolute Position Usecases

1. `ptz_set_absolute`
- 目標角度 -> 内部座標へ逆写像
- 収束まで `GetPtzCurPos` をポーリングし閉ループ
2. `ptz_get_absolute`
- 現在内部座標 -> キャリブレーション写像で角度返却

### 5. Interfaces (CLI/MCP)

1. CLI 追加
- `reocli ptz calibrate auto [--channel]`
- `reocli ptz set-absolute <pan_deg> <tilt_deg> [--tol-deg] [--timeout-ms] [--channel]`
- `reocli ptz get-absolute [--channel]`
2. MCP 追加
- `reolink.ptz_calibrate_auto`
- `reolink.ptz_set_absolute`
- `reolink.ptz_get_absolute`

### 6. Tests

1. 単体テスト
- 軸状態更新
- `u -> speed/direction` 量子化
- キャリブレーション推定
2. 統合テスト（mockito）
- 収束/停止/タイムアウト
- キャリブレーション保存・読込
3. CLI/MCP スモーク
- 引数バリデーション
- 期待レスポンス JSON

### 7. Docs

1. `README.md` に新コマンド例を追記
2. `docs/ptz-absolute-control-spec.md` と実装差分を同期

## Milestones

1. M1: Domain + Controller core（ユニットテスト通過）
2. M2: Auto calibration usecase（保存/復元まで）
3. M3: Absolute set/get usecase（統合テスト通過）
4. M4: CLI/MCP + docs + full test gate

## Done Criteria

- `cargo check --all-targets --all-features`
- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-targets --all-features`
- 追加受け入れ確認
  - `set-absolute` が許容誤差内で停止
  - 失敗時に必ず `Stop` が送出される
  - `calibrate auto` 後に `get-absolute` が再現性を持つ

## Risks

1. 公開仕様に絶対指向精度がない
- 対策: まず暫定閾値で実装し、実機測定で閾値を更新
2. 機種/FW差分で可動域・応答が異なる
- 対策: `model/firmware` 単位でパラメータ管理
3. API が方向速度指令のみ
- 対策: ポーリング周期と制御ゲインを保守的に開始

## Dependencies

- 既存 PTZ MVP 実装（archive plan `2026-02-26_cgi-ptz-implementation`）
- `docs/ptz-absolute-control-spec.md`

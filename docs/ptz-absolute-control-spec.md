# PTZ 絶対位置制御 + 自動キャリブレーション仕様（ドラフト）

最終更新: 2026-02-26

## 1. スコープ

- 対象軸: `Pan/Tilt` のみ（Zoom は対象外）
- 座標系: `境地基準`（本仕様では「設置現場に固定されたローカル座標系」と定義）
- センサ前提: 外部センサなし。カメラ API から取得できる状態のみ使用
- キャリブレーション: 自動実行

## 2. 背景（実装/機体制約）

- 現状 API で取得可能な主状態（本リポジトリ実装）
  - `GetPtzCurPos`: `pan_position`, `tilt_position`, 各レンジ
  - `GetPtzCheckState`: `calibration_state`（`2` を calibrated 扱い）
  - `GetPtzPreset`: プリセット情報
- 現状 API の PT 操作は `PtzCtrl` による方向/速度/停止ベースで、真の「角度 absolute 指令」は未提供
- よって絶対位置制御は、状態フィードバックの閉ループで実現する

## 3. 機体仕様（Web 参照値）

モデルで可動域・速度が異なる。`GetDevInfo.model` で分岐させる。

| モデル | Pan/Tilt 範囲 | Pan 速度 | Tilt 速度 | プリセット |
|---|---|---|---|---|
| RLC-823A | Pan 360°, Tilt 0°–90° | 2.5°–150°/s | 1.5°–60°/s | 64 |
| TrackMix PoE | Pan 355°, Tilt 0°–90° | 2.5°–90°/s | 1.5°–60°/s | 最大 64（Guard 1 + Patrol Points） |
| E1 Outdoor | Pan 355°, Tilt 50° | 公開値なし | 公開値なし | 最大 64（Guard 1 + Preset） |

注記:
- Reolink 公開情報では「絶対指向精度（deg）」の明示値は確認できなかった。
- そのため本仕様の精度要件は、キャリブレーション後の実測検証で確定する。

## 4. 境地基準（ローカル固定座標）

- 原点: キャリブレーション時に決定するホーム姿勢（Guard Point）
- `pan_deg = 0`: ホーム方位
- `tilt_deg = 0`: ホーム仰角
- 正方向:
  - `pan_deg > 0`: 右回転
  - `tilt_deg > 0`: 上向き
- 範囲:
  - モデルごとの公称範囲を上限としてソフト制限

## 5. 状態方程式モデル（離散時間）

サンプリング周期 `Ts = 0.05 s`（20 Hz）

軸 `a ∈ {pan, tilt}` ごとに

- 状態:
  - `x_a[k] = [q_a[k], dq_a[k], b_a[k]]^T`
  - `q_a`: カメラ内部位置（API 生値 or 校正後角度）
  - `dq_a`: 角速度
  - `b_a`: バイアス/ドリフト吸収項
- 入力:
  - `u_a[k] ∈ [-1, 1]`（方向付き正規化速度）
- 観測:
  - `y_a[k] = q_a_meas[k]`（`GetPtzCurPos` 由来）

モデル:

`x_a[k+1] = A_a x_a[k] + B_a u_a[k] + w_a[k]`

`y_a[k] = C_a x_a[k] + v_a[k]`

推奨初期形:

- `A_a = [[1, Ts, 0], [0, alpha_a, 0], [0, 0, 1]]`
- `B_a = [[0], [beta_a], [0]]`
- `C_a = [1, 0, 1]`

`alpha_a`, `beta_a` は自動同定（最小二乗）で更新。

## 6. 絶対位置制御仕様

### 6.1 外部 I/F（追加予定）

- CLI
  - `reocli ptz calibrate auto [--channel <0-255>]`
  - `reocli ptz set-absolute <pan_deg> <tilt_deg> [--tol-deg <f64>] [--timeout-ms <u64>] [--channel <0-255>]`
  - `reocli ptz get-absolute [--channel <0-255>]`
- MCP
  - `reolink.ptz_calibrate_auto`
  - `reolink.ptz_set_absolute`
  - `reolink.ptz_get_absolute`

### 6.2 制御ループ

1. 目標 `(pan_deg, tilt_deg)` を校正写像 `f^{-1}` で内部座標へ変換
2. `GetPtzCurPos` で観測、オブザーバ（Kalman もしくは alpha-beta）で `x_hat` 推定
3. 状態フィードバック + 積分補償で `u` 計算
4. `u` を `PtzCtrl(op, speed)` に量子化
   - `|u| -> speed 1..64`
   - 符号で方向（left/right/up/down）
5. 許容誤差内で `PtzCtrl(Stop)`
6. タイムアウト時は必ず停止してエラー返却

## 7. 自動キャリブレーション仕様

### 7.1 実行トリガ

- 起動時（`calibration_state != 2`）
- 明示要求（`ptz calibrate auto`）
- 継続的な追従誤差悪化を検出したとき

### 7.2 手順

1. 事前チェック: `GetAbility` で必要コマンド確認
2. PT 可動域推定
   - 低速で端まで走査して `q_min/q_max` を記録（Pan/Tilt）
3. スケール同定
   - 公称範囲（モデル spec）と `q` 範囲から一次写像初期値を生成
4. バックラッシュ同定
   - 正逆方向の微小往復で死帯幅 `deadband_a` を推定
5. パラメータ保存
   - `offset`, `scale`, `deadband`, `alpha/beta`, 作成時刻, モデル/FW
6. 検証移動
   - 複数基準点へ移動し誤差統計を記録

### 7.3 実行中ガード

- キャリブレーション中は絶対移動コマンドを reject（Busy）
- 失敗時は安全停止し、前回有効パラメータへロールバック

## 8. 受け入れ基準（暫定）

公開精度仕様がないため暫定値。実機評価で更新する。

- 定常誤差（95パーセンタイル）
  - `|e_pan| <= 1.0°`, `|e_tilt| <= 1.0°`
- 整定時間（代表ステップ）
  - Pan 30° / Tilt 15° 指令で `<= 1.2 s`
- 再現性
  - 再起動 + 自動キャリブレーション後の同一点復帰誤差 `<= 1.5°`
- 失敗安全
  - 例外/タイムアウト時に必ず `Stop` を送出

## 9. 既知リスク

- メーカー公開情報に絶対指向精度がないため、最終要求は実機実測依存
- モデル/FW 差分が大きく、同一係数を全機種共通化できない
- 外部センサなしのため、絶対方位（真北など）への直接拘束はできない

## 10. 参考ソース

- Reolink RLC-823A 仕様（Pan/Tilt 範囲・速度・Preset）
  - https://reolink.com/product/rlc-823a/
- Reolink TrackMix PoE 仕様（Pan/Tilt 範囲・速度・Preset）
  - https://reolink.com/product/reolink-trackmix-poe/
- Reolink E1 Outdoor 仕様（Pan/Tilt 範囲・Preset）
  - https://reolink.com/product/e1-outdoor/
- Reolink サポート: PTZ Camera Calibration（アプリでの Calibration 操作）
  - https://support.reolink.com/hc/en-us/articles/34512745835289-Introduction-to-PTZ-Camera-Calibration/
- Reolink サポート: Modify Monitor Point（PTZ 校正中は操作不可の注意）
  - https://support.reolink.com/hc/en-us/articles/360008718814-How-to-Set-up-Monitor-Point-via-Reolink-App/


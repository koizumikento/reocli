#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
DEFAULT_BIN="$REPO_ROOT/target/debug/reocli"

REOCLI_BIN="${REOCLI_BIN:-$DEFAULT_BIN}"
ENDPOINT="${REOCLI_ENDPOINT:-https://camera.local}"
USER_NAME="${REOCLI_USER:-admin}"
PASSWORD="${REOCLI_PASSWORD:-}"
BACKEND="${REOCLI_PTZ_BACKEND:-onvif}"
CHANNEL="${CHANNEL:-0}"
TOL_COUNT="${TOL_COUNT:-12}"
TIMEOUT_MS="${TIMEOUT_MS:-25000}"
ROUND_DIR="${ROUND_DIR:-}"
ROUND_COUNT="${ROUND_COUNT:-5}"
STRICT_PAN_MAX="${STRICT_PAN_MAX:-50}"
STRICT_TILT_MAX="${STRICT_TILT_MAX:-24}"
SKIP_CALIB="${SKIP_CALIB:-0}"
RESET_EKF="${RESET_EKF:-0}"
ISOLATE_STATE="${ISOLATE_STATE:-auto}"
# modes: return_only | post_get_once | post_get_stable | post_get_stable_median
SETTLE_EVAL_MODE="${SETTLE_EVAL_MODE:-return_only}"
SETTLE_POLL_MAX="${SETTLE_POLL_MAX:-8}"
SETTLE_STABLE_HITS="${SETTLE_STABLE_HITS:-1}"
SETTLE_SLEEP_MS="${SETTLE_SLEEP_MS:-120}"
SETTLE_SLEEP_SEC=$(awk "BEGIN { printf \"%.3f\", ${SETTLE_SLEEP_MS}/1000 }")

if [ ! -x "$REOCLI_BIN" ]; then
  echo "reocli binary not found or not executable: $REOCLI_BIN" >&2
  echo "build with: cargo build --bin reocli" >&2
  exit 1
fi

if [ -z "$ROUND_DIR" ]; then
  echo "ROUND_DIR is required (directory containing random_round_1.tsv ...)." >&2
  exit 1
fi

if [ ! -d "$ROUND_DIR" ]; then
  echo "ROUND_DIR does not exist: $ROUND_DIR" >&2
  exit 1
fi

OUT_DIR="${OUT_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/reocli-ekf-reval.XXXXXX")}"
mkdir -p "$OUT_DIR"
RESULTS_TSV="$OUT_DIR/random_results.tsv"
SUMMARY_TXT="$OUT_DIR/random_summary.txt"
CALIB_OUT="$OUT_DIR/calibrate.out"

resolve_state_dir() {
  local explicit="${REOCLI_CALIBRATION_DIR:-}"
  if [ -n "$explicit" ]; then
    echo "$explicit"
    return
  fi

  case "$ISOLATE_STATE" in
    auto)
      if [ "$SKIP_CALIB" != "1" ]; then
        echo "$OUT_DIR/reocli_state"
      fi
      ;;
    1|true|yes)
      echo "$OUT_DIR/reocli_state"
      ;;
    0|false|no)
      ;;
    *)
      echo "invalid ISOLATE_STATE: $ISOLATE_STATE" >&2
      exit 1
      ;;
  esac
}

STATE_DIR="$(resolve_state_dir)"
if [ -n "$STATE_DIR" ]; then
  mkdir -p "$STATE_DIR"
fi

sanitize_key_component() {
  local raw="$1"
  local key
  key=$(
    printf '%s' "$raw" |
      tr '[:upper:]' '[:lower:]' |
      sed -E 's/[^[:alnum:]]+/_/g; s/^_+//; s/_+$//'
  )
  if [ -z "$key" ]; then
    echo "unknown"
  else
    echo "$key"
  fi
}

effective_calibration_dir() {
  if [ -n "$STATE_DIR" ]; then
    echo "$STATE_DIR"
    return
  fi
  if [ -n "${REOCLI_CALIBRATION_DIR:-}" ]; then
    echo "$REOCLI_CALIBRATION_DIR"
    return
  fi
  if [ -n "${HOME:-}" ]; then
    echo "$HOME/.reocli/calibration"
  else
    echo ".reocli/calibration"
  fi
}

run_reocli() {
  if [ -n "$STATE_DIR" ]; then
    REOCLI_ENDPOINT="$ENDPOINT" \
      REOCLI_USER="$USER_NAME" \
      REOCLI_PASSWORD="$PASSWORD" \
      REOCLI_PTZ_BACKEND="$BACKEND" \
      REOCLI_CALIBRATION_DIR="$STATE_DIR" \
      "$REOCLI_BIN" "$@"
  else
    REOCLI_ENDPOINT="$ENDPOINT" \
      REOCLI_USER="$USER_NAME" \
      REOCLI_PASSWORD="$PASSWORD" \
      REOCLI_PTZ_BACKEND="$BACKEND" \
      "$REOCLI_BIN" "$@"
  fi
}

abs() {
  local v="$1"
  if [ "$v" -lt 0 ]; then
    echo $((-v))
  else
    echo "$v"
  fi
}

now_ms() {
  python3 - <<'PY'
import time
print(int(time.time()*1000))
PY
}

stats_line() {
  local infile="$1"
  local label="$2"
  local sorted
  sorted=$(awk 'NF>0{print $1}' "$infile" | sort -n)
  local n
  n=$(printf '%s\n' "$sorted" | sed '/^$/d' | wc -l | tr -d ' ')
  if [ "$n" -eq 0 ]; then
    echo "${label}_n=0 median=0 p95=0"
    return
  fi
  local mid p95 median p95v
  mid=$(((n + 1) / 2))
  p95=$(((95 * n + 99) / 100))
  [ "$p95" -lt 1 ] && p95=1
  [ "$p95" -gt "$n" ] && p95=$n
  median=$(printf '%s\n' "$sorted" | sed -n "${mid}p")
  p95v=$(printf '%s\n' "$sorted" | sed -n "${p95}p")
  echo "${label}_n=${n} median=${median} p95=${p95v}"
}

stats_sum_line() {
  local infile="$1"
  local label="$2"
  local sorted
  sorted=$(awk 'NF>0{print $1}' "$infile" | sort -n)
  local n
  n=$(printf '%s\n' "$sorted" | sed '/^$/d' | wc -l | tr -d ' ')
  if [ "$n" -eq 0 ]; then
    echo "${label}_n=0 median=0 p95=0 mean=0"
    return
  fi
  local mid p95 median p95v mean
  mid=$(((n + 1) / 2))
  p95=$(((95 * n + 99) / 100))
  [ "$p95" -lt 1 ] && p95=1
  [ "$p95" -gt "$n" ] && p95=$n
  median=$(printf '%s\n' "$sorted" | sed -n "${mid}p")
  p95v=$(printf '%s\n' "$sorted" | sed -n "${p95}p")
  mean=$(awk 'NF>0{s+=$1; n+=1} END{if(n==0){print 0}else{printf "%.1f", s/n}}' "$infile")
  echo "${label}_n=${n} median=${median} p95=${p95v} mean=${mean}"
}

median_from_lines() {
  local lines="$1"
  local sorted
  sorted=$(printf '%s\n' "$lines" | sed '/^$/d' | sort -n)
  local n
  n=$(printf '%s\n' "$sorted" | sed '/^$/d' | wc -l | tr -d ' ')
  if [ "$n" -eq 0 ]; then
    echo ""
    return
  fi
  local mid
  mid=$(((n + 1) / 2))
  printf '%s\n' "$sorted" | sed -n "${mid}p"
}

echo -e "round\tseq\ttarget_pan\ttarget_tilt\tset_status\telapsed_ms\tfinal_pan\tfinal_tilt\tpan_abs_err\ttilt_abs_err\tset_output\tret_pan\tret_tilt\tobs_pan\tobs_tilt\teval_mode\tsettle_polls\tret_pan_abs_err\tret_tilt_abs_err" > "$RESULTS_TSV"

if [ "$RESET_EKF" = "1" ]; then
  calib_dir="$(effective_calibration_dir)"
  endpoint_key="$(sanitize_key_component "$ENDPOINT")"
  ekf_path="${calib_dir}/${endpoint_key}.ch${CHANNEL}.ekf-count.json"
  rm -f "$ekf_path"
  echo "ekf_state_reset=1 path=$ekf_path"
fi

if [ "$SKIP_CALIB" != "1" ]; then
  set +e
  calib_out=$(run_reocli ptz calibrate auto --channel "$CHANNEL" 2>&1)
  calib_rc=$?
  set -e
  printf '%s\n' "$calib_out" > "$CALIB_OUT"
  if [ "$calib_rc" -ne 0 ]; then
    echo "calibration failed rc=$calib_rc"
    echo "$calib_out"
    exit "$calib_rc"
  fi
  echo "$calib_out"
else
  echo "skipped calibration (SKIP_CALIB=1)"
fi

for round in $(seq 1 "$ROUND_COUNT"); do
  round_file="$ROUND_DIR/random_round_${round}.tsv"
  if [ ! -f "$round_file" ]; then
    echo "missing round file: $round_file" >&2
    exit 1
  fi
  echo "round=${round} start"
  seq=0
  while IFS=$'\t' read -r target_pan target_tilt; do
    [ -z "${target_pan:-}" ] && continue
    if ! [[ "$target_pan" =~ ^-?[0-9]+$ ]] || ! [[ "$target_tilt" =~ ^-?[0-9]+$ ]]; then
      continue
    fi
    seq=$((seq + 1))

    start_ms=$(now_ms)
    set +e
    set_out=$(run_reocli ptz set-absolute "$target_pan" "$target_tilt" \
      --tol-count "$TOL_COUNT" --timeout-ms "$TIMEOUT_MS" --channel "$CHANNEL" 2>&1)
    set_rc=$?
    set -e
    end_ms=$(now_ms)
    elapsed_ms=$((end_ms - start_ms))

    ret_pan=$(printf '%s\n' "$set_out" | sed -n 's/.*pan_count=\([-0-9]*\).*/\1/p' | tail -n1)
    ret_tilt=$(printf '%s\n' "$set_out" | sed -n 's/.*tilt_count=\([-0-9]*\).*/\1/p' | tail -n1)
    final_pan="$ret_pan"
    final_tilt="$ret_tilt"
    obs_pan="$ret_pan"
    obs_tilt="$ret_tilt"
    if [ -z "$final_pan" ] || [ -z "$final_tilt" ]; then
      final_pan=0
      final_tilt=0
      obs_pan=0
      obs_tilt=0
    fi

    settle_polls=0
    case "$SETTLE_EVAL_MODE" in
      return_only)
        ;;
      post_get_once)
        settle_polls=1
        pos_out=$(run_reocli ptz get-absolute --channel "$CHANNEL" 2>&1 || true)
        cand_pan=$(printf '%s\n' "$pos_out" | sed -n 's/.*pan_count=\([-0-9]*\).*/\1/p' | tail -n1)
        cand_tilt=$(printf '%s\n' "$pos_out" | sed -n 's/.*tilt_count=\([-0-9]*\).*/\1/p' | tail -n1)
        if [ -n "$cand_pan" ] && [ -n "$cand_tilt" ]; then
          obs_pan="$cand_pan"
          obs_tilt="$cand_tilt"
        fi
        ;;
      post_get_stable)
        prev_pan=""
        prev_tilt=""
        stable_hits=0
        for poll in $(seq 1 "$SETTLE_POLL_MAX"); do
          sleep "$SETTLE_SLEEP_SEC"
          settle_polls=$poll
          pos_out=$(run_reocli ptz get-absolute --channel "$CHANNEL" 2>&1 || true)
          cand_pan=$(printf '%s\n' "$pos_out" | sed -n 's/.*pan_count=\([-0-9]*\).*/\1/p' | tail -n1)
          cand_tilt=$(printf '%s\n' "$pos_out" | sed -n 's/.*tilt_count=\([-0-9]*\).*/\1/p' | tail -n1)
          if [ -z "$cand_pan" ] || [ -z "$cand_tilt" ]; then
            continue
          fi
          obs_pan="$cand_pan"
          obs_tilt="$cand_tilt"
          if [ "$prev_pan" = "$cand_pan" ] && [ "$prev_tilt" = "$cand_tilt" ]; then
            stable_hits=$((stable_hits + 1))
          else
            stable_hits=0
          fi
          prev_pan="$cand_pan"
          prev_tilt="$cand_tilt"
          if [ "$stable_hits" -ge "$SETTLE_STABLE_HITS" ]; then
            break
          fi
        done
        ;;
      post_get_stable_median)
        prev_pan=""
        prev_tilt=""
        stable_hits=0
        last_valid_pan=""
        last_valid_tilt=""
        stable_window_pan=""
        stable_window_tilt=""
        for poll in $(seq 1 "$SETTLE_POLL_MAX"); do
          sleep "$SETTLE_SLEEP_SEC"
          settle_polls=$poll
          pos_out=$(run_reocli ptz get-absolute --channel "$CHANNEL" 2>&1 || true)
          cand_pan=$(printf '%s\n' "$pos_out" | sed -n 's/.*pan_count=\([-0-9]*\).*/\1/p' | tail -n1)
          cand_tilt=$(printf '%s\n' "$pos_out" | sed -n 's/.*tilt_count=\([-0-9]*\).*/\1/p' | tail -n1)
          if [ -z "$cand_pan" ] || [ -z "$cand_tilt" ]; then
            continue
          fi

          obs_pan="$cand_pan"
          obs_tilt="$cand_tilt"
          last_valid_pan="$cand_pan"
          last_valid_tilt="$cand_tilt"

          if [ "$prev_pan" = "$cand_pan" ] && [ "$prev_tilt" = "$cand_tilt" ]; then
            stable_hits=$((stable_hits + 1))
            stable_window_pan=$(printf '%s\n%s\n' "$stable_window_pan" "$cand_pan")
            stable_window_tilt=$(printf '%s\n%s\n' "$stable_window_tilt" "$cand_tilt")
          else
            stable_hits=0
            stable_window_pan="$cand_pan"
            stable_window_tilt="$cand_tilt"
          fi

          prev_pan="$cand_pan"
          prev_tilt="$cand_tilt"
          if [ "$stable_hits" -ge "$SETTLE_STABLE_HITS" ]; then
            break
          fi
        done

        med_pan=$(median_from_lines "$stable_window_pan")
        med_tilt=$(median_from_lines "$stable_window_tilt")
        if [ -n "$med_pan" ] && [ -n "$med_tilt" ]; then
          obs_pan="$med_pan"
          obs_tilt="$med_tilt"
        elif [ -n "$last_valid_pan" ] && [ -n "$last_valid_tilt" ]; then
          obs_pan="$last_valid_pan"
          obs_tilt="$last_valid_tilt"
        fi
        ;;
      *)
        echo "invalid SETTLE_EVAL_MODE: $SETTLE_EVAL_MODE" >&2
        exit 1
        ;;
    esac

    if [ -z "$obs_pan" ] || [ -z "$obs_tilt" ]; then
      obs_pan="$final_pan"
      obs_tilt="$final_tilt"
    fi

    pan_abs_err=$(abs $((obs_pan - target_pan)))
    tilt_abs_err=$(abs $((obs_tilt - target_tilt)))
    ret_pan_abs_err=$(abs $((final_pan - target_pan)))
    ret_tilt_abs_err=$(abs $((final_tilt - target_tilt)))

    safe_set_out=$(printf '%s' "$set_out" | tr '\t\n' '  ')
    echo -e "${round}\t${seq}\t${target_pan}\t${target_tilt}\t${set_rc}\t${elapsed_ms}\t${final_pan}\t${final_tilt}\t${pan_abs_err}\t${tilt_abs_err}\t${safe_set_out}\t${ret_pan}\t${ret_tilt}\t${obs_pan}\t${obs_tilt}\t${SETTLE_EVAL_MODE}\t${settle_polls}\t${ret_pan_abs_err}\t${ret_tilt_abs_err}" >> "$RESULTS_TSV"
    printf 'round=%d seq=%02d status=%d elapsed_ms=%d obs_err=(%d,%d) ret_err=(%d,%d)\n' \
      "$round" "$seq" "$set_rc" "$elapsed_ms" "$pan_abs_err" "$tilt_abs_err" "$ret_pan_abs_err" "$ret_tilt_abs_err"
  done < "$round_file"
done

: > "$SUMMARY_TXT"
for round in $(seq 1 "$ROUND_COUNT"); do
  awk -F'\t' -v r="$round" 'NR>1 && $1==r {print $9}' "$RESULTS_TSV" > "$OUT_DIR/.tmp_pan_err_r${round}.txt"
  awk -F'\t' -v r="$round" 'NR>1 && $1==r {print $10}' "$RESULTS_TSV" > "$OUT_DIR/.tmp_tilt_err_r${round}.txt"
  awk -F'\t' -v r="$round" 'NR>1 && $1==r {print $6}' "$RESULTS_TSV" > "$OUT_DIR/.tmp_elapsed_ms_r${round}.txt"
  awk -F'\t' -v r="$round" 'NR>1 && $1==r {print $9+$10}' "$RESULTS_TSV" > "$OUT_DIR/.tmp_sum_r${round}.txt"
  awk -F'\t' -v r="$round" 'NR>1 && $1==r && $5==0 {print $9}' "$RESULTS_TSV" > "$OUT_DIR/.tmp_pan_err_ok_r${round}.txt"
  awk -F'\t' -v r="$round" 'NR>1 && $1==r && $5==0 {print $10}' "$RESULTS_TSV" > "$OUT_DIR/.tmp_tilt_err_ok_r${round}.txt"
  awk -F'\t' -v r="$round" 'NR>1 && $1==r && $5==0 {print $9+$10}' "$RESULTS_TSV" > "$OUT_DIR/.tmp_sum_ok_r${round}.txt"
  total=$(awk -F'\t' -v r="$round" 'NR>1 && $1==r {n++} END{print n+0}' "$RESULTS_TSV")
  success=$(awk -F'\t' -v r="$round" 'NR>1 && $1==r && $5==0 {n++} END{print n+0}' "$RESULTS_TSV")
  strict_success=$(awk -F'\t' -v r="$round" -v sp="$STRICT_PAN_MAX" -v st="$STRICT_TILT_MAX" \
    'NR>1 && $1==r && $5==0 && $9<=sp && $10<=st {n++} END{print n+0}' "$RESULTS_TSV")
  timeout_count=$(awk -F'\t' -v r="$round" 'NR>1 && $1==r && $11 ~ /set_absolute_raw timeout/ {n++} END{print n+0}' "$RESULTS_TSV")
  failures=$((total - success))
  echo "round${round} total=${total} success=${success} failures=${failures} timeout_count=${timeout_count}" | tee -a "$SUMMARY_TXT"
  echo "round${round} strict_success=${strict_success}/${total} strict_pan_max=${STRICT_PAN_MAX} strict_tilt_max=${STRICT_TILT_MAX}" | tee -a "$SUMMARY_TXT"
  stats_line "$OUT_DIR/.tmp_pan_err_r${round}.txt" "pan_err_r${round}" | tee -a "$SUMMARY_TXT"
  stats_line "$OUT_DIR/.tmp_tilt_err_r${round}.txt" "tilt_err_r${round}" | tee -a "$SUMMARY_TXT"
  stats_line "$OUT_DIR/.tmp_elapsed_ms_r${round}.txt" "elapsed_ms_r${round}" | tee -a "$SUMMARY_TXT"
  stats_sum_line "$OUT_DIR/.tmp_sum_r${round}.txt" "sum_err_r${round}" | tee -a "$SUMMARY_TXT"
  stats_sum_line "$OUT_DIR/.tmp_pan_err_ok_r${round}.txt" "pan_err_ok_r${round}" | tee -a "$SUMMARY_TXT"
  stats_sum_line "$OUT_DIR/.tmp_tilt_err_ok_r${round}.txt" "tilt_err_ok_r${round}" | tee -a "$SUMMARY_TXT"
  stats_sum_line "$OUT_DIR/.tmp_sum_ok_r${round}.txt" "sum_err_ok_r${round}" | tee -a "$SUMMARY_TXT"
done

awk -F'\t' 'NR>1 && $1>=2 {print $9}' "$RESULTS_TSV" > "$OUT_DIR/.tmp_pan_warm.txt"
awk -F'\t' 'NR>1 && $1>=2 {print $10}' "$RESULTS_TSV" > "$OUT_DIR/.tmp_tilt_warm.txt"
awk -F'\t' 'NR>1 && $1>=2 {print $6}' "$RESULTS_TSV" > "$OUT_DIR/.tmp_elapsed_warm.txt"
awk -F'\t' 'NR>1 && $1>=2 {print $9+$10}' "$RESULTS_TSV" > "$OUT_DIR/.tmp_sum_warm.txt"
awk -F'\t' 'NR>1 && $1>=2 && $5==0 {print $9}' "$RESULTS_TSV" > "$OUT_DIR/.tmp_pan_warm_ok.txt"
awk -F'\t' 'NR>1 && $1>=2 && $5==0 {print $10}' "$RESULTS_TSV" > "$OUT_DIR/.tmp_tilt_warm_ok.txt"
awk -F'\t' 'NR>1 && $1>=2 && $5==0 {print $9+$10}' "$RESULTS_TSV" > "$OUT_DIR/.tmp_sum_warm_ok.txt"
strict_success_warm=$(awk -F'\t' -v sp="$STRICT_PAN_MAX" -v st="$STRICT_TILT_MAX" \
  'NR>1 && $1>=2 && $5==0 && $9<=sp && $10<=st {n++} END{print n+0}' "$RESULTS_TSV")
total_warm=$(awk -F'\t' 'NR>1 && $1>=2 {n++} END{print n+0}' "$RESULTS_TSV")
timeout_warm=$(awk -F'\t' 'NR>1 && $1>=2 && $11 ~ /set_absolute_raw timeout/ {n++} END{print n+0}' "$RESULTS_TSV")
echo "warm_strict_success=${strict_success_warm}/${total_warm} strict_pan_max=${STRICT_PAN_MAX} strict_tilt_max=${STRICT_TILT_MAX}" | tee -a "$SUMMARY_TXT"
echo "warm_timeout_count=${timeout_warm}" | tee -a "$SUMMARY_TXT"

stats_sum_line "$OUT_DIR/.tmp_pan_warm.txt" "pan_err_warm" | tee -a "$SUMMARY_TXT"
stats_sum_line "$OUT_DIR/.tmp_tilt_warm.txt" "tilt_err_warm" | tee -a "$SUMMARY_TXT"
stats_sum_line "$OUT_DIR/.tmp_sum_warm.txt" "sum_err_warm" | tee -a "$SUMMARY_TXT"
stats_sum_line "$OUT_DIR/.tmp_elapsed_warm.txt" "elapsed_ms_warm" | tee -a "$SUMMARY_TXT"
stats_sum_line "$OUT_DIR/.tmp_pan_warm_ok.txt" "pan_err_warm_ok" | tee -a "$SUMMARY_TXT"
stats_sum_line "$OUT_DIR/.tmp_tilt_warm_ok.txt" "tilt_err_warm_ok" | tee -a "$SUMMARY_TXT"
stats_sum_line "$OUT_DIR/.tmp_sum_warm_ok.txt" "sum_err_warm_ok" | tee -a "$SUMMARY_TXT"

echo "REVAL_RESULT_DIR=$OUT_DIR"
echo "REVAL_RESULTS_TSV=$RESULTS_TSV"
echo "REVAL_SUMMARY_TXT=$SUMMARY_TXT"
if [ -n "$STATE_DIR" ]; then
  echo "REVAL_STATE_DIR=$STATE_DIR"
fi

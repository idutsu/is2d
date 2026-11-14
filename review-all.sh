#!/bin/sh
# is2d のゼロベースレビューを、Claude（Claude Code）と Gemini（Antigravity CLI）で1つずつ順に行い、最後に Claude が
# 2つの報告書を1つの課題リストと感想にまとめる。コードは直さない（直すのは、まとめを読んで返事をしてから）。
#
# 使い方：リポジトリの中で `sh review-all.sh`
#   - 対象は、今いるブランチが追いかけている GitHub のブランチの最新（review-prompt.md と同じ決め方）
#   - 2つのレビューは同時に動かさない（同じ Mac で計測がぶつかり、時間が当てにならないため）
#   - 進み具合は claude/review-logs/<コミット>/progress.log と、Mac の通知に出る
#
# 環境変数（どれも無くてよい）
#   AGY_MODEL    Antigravity で使うモデル（`agy models` で名前を確かめて入れる。空なら Antigravity の既定のモデル）
#   AGY_TIMEOUT  Antigravity の待ち時間（既定 1440m）
#
# 注意：人のいない所で最後まで進めるため、どちらの AI もコマンドとファイルの書き込みを自動で承認して動かす
#   （--dangerously-skip-permissions）。レビューとまとめは、それぞれ別の作業場所（git worktree）で行う決まり。
set -eu

cd "$(git rev-parse --show-toplevel)"
ROOT=$(pwd)
git fetch origin
C=$(git rev-parse --short=7 '@{upstream}')
LOG="$ROOT/claude/review-logs/$C"
mkdir -p "$LOG"

note() {
    printf '%s %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$1" | tee -a "$LOG/progress.log"
    if command -v osascript >/dev/null 2>&1; then
        osascript -e "display notification \"$1\" with title \"is2d レビュー\"" >/dev/null 2>&1 || true
    fi
}

# claude/review-<C>-<名前>/ が無い名前（あれば -2、-3… を付ける）
free_name() {
    name=$1
    n=1
    while [ -e "$ROOT/claude/review-$C-$name" ]; do
        n=$((n + 1))
        name="$1-$n"
    done
    printf '%s' "$name"
}

review_request() {
    printf '%s' "$ROOT/review-prompt.md を読み、そのとおりにレビューを進めてください。準備の「自分の印を決める」では、印を $1 にしてください（番号を付けない）。人のいない所で動いているので、利用者に質問せず、止まるときはその理由を最後の報告に書いて終わってください。"
}

need() {
    if [ ! -f "$1" ]; then
        note "止めました：$1 ができませんでした（記録：$2）"
        exit 1
    fi
}

CL=$(free_name claude)
GE=$(free_name gemini)
MG=$(free_name merged)
note "対象コミット $C。Claude のレビューを始めます（claude/review-$C-$CL/）"
claude -p "$(review_request "$CL")" --model opus --dangerously-skip-permissions \
    >"$LOG/claude.log" 2>&1 || true
need "$ROOT/claude/review-$C-$CL/issues.md" "$LOG/claude.log"

note "Gemini のレビューを始めます（claude/review-$C-$GE/）"
# shellcheck disable=SC2086
agy -p "$(review_request "$GE")" ${AGY_MODEL:+--model "$AGY_MODEL"} --dangerously-skip-permissions \
    --print-timeout "${AGY_TIMEOUT:-1440m}" >"$LOG/gemini.log" 2>&1 || true
need "$ROOT/claude/review-$C-$GE/issues.md" "$LOG/gemini.log"

note "2つの報告書をまとめます（claude/review-$C-$MG/）"
claude -p "$ROOT/review-merge-prompt.md を読み、そのとおりにまとめてください。対象コミットは $C、報告の置き場は $ROOT/claude/review-$C-$CL/（Claude）と $ROOT/claude/review-$C-$GE/（Gemini）、まとめの置き場は $ROOT/claude/review-$C-$MG/ です（まだ無ければ作ってください）。人のいない所で動いているので、利用者に質問せず、止まるときはその理由を最後の報告に書いて終わってください。" \
    --model opus --dangerously-skip-permissions >"$LOG/merge.log" 2>&1 || true
need "$ROOT/claude/review-$C-$MG/summary.md" "$LOG/merge.log"

note "終わりました。まとめ：claude/review-$C-$MG/summary.md"

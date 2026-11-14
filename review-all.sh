#!/bin/sh
# is2d のゼロベースレビューを、Claude（Claude Code）と Gemini（Antigravity CLI）で1つずつ順に行い、最後に Claude が
# 2つの報告書を1つの課題リストと感想にまとめる。コードは直さない（直すのは、まとめを読んで返事をしてから）。
#
# 使い方：リポジトリの中で `sh review-all.sh`
#   - 始めに手元を GitHub の最新に追いつかせ（git pull --ff-only）、そのコミットをレビューする。レビューもまとめも、
#     このリポジトリのフォルダで行う。保存していない変更があるときは、何もせずに止まる（消さないため）
#   - 2つのレビューは同時に動かさない（同じ Mac で計測がぶつかり、時間が当てにならないため）
#   - 進み具合は claude/review-logs/<コミット>/progress.log と、Mac の通知に出る
#
# 環境変数（どれも無くてよい）
#   AGY_MODEL    Antigravity で使うモデル（`agy models` で名前を確かめて入れる。空なら Antigravity の既定のモデル）
#   AGY_TIMEOUT  Antigravity の待ち時間（既定 1440m）
#   IS2D_LETTER  利用者への手紙を書くファイル（review-loop/loop.sh が渡す。あれば、まとめのあとに手紙も書かせる）
#   IS2D_REPLY   利用者が返事を書くファイル（手紙に書かせるため。IS2D_LETTER と一緒に渡す）
#
# 終わると、claude/review-logs/<コミット>/summary-path に、まとめ（summary.md）のパスを書く
# 同じコミットで、もう終わっているレビュー（issues.md がある）があれば、やり直さずに使い回す
#
# 注意：人のいない所で最後まで進めるため、どちらの AI もコマンドとファイルの書き込みを自動で承認して動かす
#   （--dangerously-skip-permissions）。
set -eu

cd "$(git rev-parse --show-toplevel)"
ROOT=$(pwd)
if [ -n "$(git status --porcelain)" ]; then
    echo "このフォルダに保存していない変更があるので、始めません（消さないため）。" >&2
    exit 1
fi
git pull -q --ff-only
C=$(git rev-parse --short=7 HEAD)
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

# 同じコミットで、もう終わっているレビューの印（issues.md がある置き場のうち、いちばん新しいもの）。あれば使い回す
# （どこかの段で止まってやり直すときに、何時間もかかる終わったレビューを繰り返さないため）
finished() {
    for d in $(ls -dt "$ROOT/claude/review-$C-$1" "$ROOT/claude/review-$C-$1"-[0-9]* 2>/dev/null); do
        if [ -f "$d/issues.md" ]; then
            basename "$d" | sed "s/^review-$C-//"
            return
        fi
    done
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

MG=$(free_name merged)
CL=$(finished claude)
if [ -n "$CL" ]; then
    note "対象コミット $C。Claude のレビューは終わっているので使い回します（claude/review-$C-$CL/）"
else
    CL=$(free_name claude)
    note "対象コミット $C。Claude のレビューを始めます（claude/review-$C-$CL/）"
    claude -p "$(review_request "$CL")" --model opus --dangerously-skip-permissions \
        >"$LOG/claude.log" 2>&1 || true
    need "$ROOT/claude/review-$C-$CL/issues.md" "$LOG/claude.log"
fi

GE=$(finished gemini)
if [ -n "$GE" ]; then
    note "Gemini のレビューは終わっているので使い回します（claude/review-$C-$GE/）"
else
    GE=$(free_name gemini)
    note "Gemini のレビューを始めます（claude/review-$C-$GE/）"
    # shellcheck disable=SC2086
    agy -p "$(review_request "$GE")" ${AGY_MODEL:+--model "$AGY_MODEL"} --dangerously-skip-permissions \
        --print-timeout "${AGY_TIMEOUT:-1440m}" >"$LOG/gemini.log" 2>&1 || true
    need "$ROOT/claude/review-$C-$GE/issues.md" "$LOG/gemini.log"
fi

LETTER_REQ=""
if [ -n "${IS2D_LETTER:-}" ]; then
    LETTER_REQ="手紙の置き場は ${IS2D_LETTER}、返事の置き場は ${IS2D_REPLY:-（手紙と同じフォルダの、手紙の名前の「手紙」を「返事」にしたもの）} です。"
fi
note "2つの報告書をまとめます（claude/review-$C-$MG/）"
claude -p "$ROOT/review-merge-prompt.md を読み、そのとおりにまとめてください。対象コミットは $C、報告の置き場は $ROOT/claude/review-$C-$CL/（Claude）と $ROOT/claude/review-$C-$GE/（Gemini）、まとめの置き場は $ROOT/claude/review-$C-$MG/ です（まだ無ければ作ってください）。$LETTER_REQ人のいない所で動いているので、利用者に質問せず、止まるときはその理由を最後の報告に書いて終わってください。" \
    --model opus --dangerously-skip-permissions >"$LOG/merge.log" 2>&1 || true
need "$ROOT/claude/review-$C-$MG/summary.md" "$LOG/merge.log"
printf '%s\n' "$ROOT/claude/review-$C-$MG/summary.md" >"$LOG/summary-path"
if [ -n "${IS2D_LETTER:-}" ]; then
    need "$IS2D_LETTER" "$LOG/merge.log"
fi

note "終わりました。まとめ：claude/review-$C-$MG/summary.md"

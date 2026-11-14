#!/bin/sh
# is2d のレビューと修正の繰り返しの「見回り」1回分。launchd が 10 分ごとに動かす（install.sh で登録する）。
# 今どの段かを claude/review-loop/ のファイルの有る無しで決め、やることがあるときだけ進める。
#   <C> は、今いるブランチが追いかけている GitHub のブランチの最新のコミット（7桁）
#
#   STOP がある                       → 何もしない
#   <C>-止まった.md がある            → 何もしない（中の理由を見て、そのファイルを消すと、次の見回りでやり直す）
#   <C>-手紙.md が無い                → レビューして、まとめ、利用者への手紙を書く（review-all.sh）
#   <C>-手紙.md だけがある            → 返事を待つ（何もしない）
#   <C>-返事.md があり、報告が無い    → 直す（Claude）→ 審査（Gemini）→ 仕上げ（Claude）→ 整形・テスト・clippy の確かめ
#                                       → プッシュ → <C>-報告.md。プッシュすると <C> が変わり、次の見回りでまたレビューする
#   <C>-報告.md がある                → 何もしない（直すものが無くプッシュしなかったときなど）
#
# 環境変数（どれも無くてよい）：AGY_MODEL・AGY_TIMEOUT（review-all.sh と同じ）
set -eu

REPO=$(cd "$(dirname "$0")/.." && pwd)
cd "$REPO"
DIR="$REPO/claude/review-loop"
mkdir -p "$DIR"
[ -e "$DIR/STOP" ] && exit 0

# 同時に2つ動かない（前の見回りが長いレビューの最中なら、何もしない）
LOCK="$DIR/.lock"
if ! mkdir "$LOCK" 2>/dev/null; then
    pid=$(cat "$LOCK/pid" 2>/dev/null || true)
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        exit 0
    fi
    rm -rf "$LOCK"
    mkdir "$LOCK"
fi
echo $$ >"$LOCK/pid"
trap 'rm -rf "$LOCK"' EXIT
trap 'exit 1' INT TERM HUP

note() {
    printf '%s %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$1" >>"$DIR/progress.log"
    if command -v osascript >/dev/null 2>&1; then
        osascript -e "display notification \"$1\" with title \"is2d\" sound name \"Glass\"" >/dev/null 2>&1 || true
    fi
}

git fetch -q origin
UP=$(git rev-parse --abbrev-ref '@{upstream}')
REMOTE=${UP%%/*}
BRANCH=${UP#*/}
FULL=$(git rev-parse '@{upstream}')
C=$(git rev-parse --short=7 '@{upstream}')
LETTER="$DIR/$C-手紙.md"
REPLY="$DIR/$C-返事.md"
REPORT="$DIR/$C-報告.md"
STOPPED="$DIR/$C-止まった.md"
LOG="$REPO/claude/review-logs/$C"

# 止まった理由を書いて、この見回りを終える（同じ所で何度もやり直さないため。ファイルを消すとやり直す）
stopped() {
    {
        printf '# 止まりました（コミット %s）\n\n' "$C"
        printf '%s\n\n' "$1"
        printf -- '- 記録：%s\n' "$2"
        printf -- '- 直す段の作業場所（あれば）：%s\n\n' "$FIX"
        printf 'このファイルを消すと、次の見回り（10 分以内）で、この段からやり直します。\n'
    } >"$STOPPED"
    note "止まりました：$1（$STOPPED）"
    exit 0
}

FIX="$REPO/../is2d-fix-$C"
BASEW="$REPO/../is2d-base-$C"
[ -e "$STOPPED" ] && exit 0
[ -e "$REPORT" ] && exit 0

if [ ! -e "$LETTER" ]; then
    note "コミット $C をレビューします（Claude と Gemini が1つずつ。何時間もかかります）"
    if IS2D_LETTER="$LETTER" IS2D_REPLY="$REPLY" caffeinate -i sh "$REPO/review-all.sh" >>"$DIR/review-all.out" 2>&1; then
        note "手紙が届きました：$LETTER"
    else
        stopped "レビューかまとめが途中で止まりました。" "$LOG/progress.log"
    fi
    exit 0
fi

[ -e "$REPLY" ] || exit 0

# ---- 返事があるので直す ----
mkdir -p "$LOG"
SUMMARY=$(cat "$LOG/summary-path" 2>/dev/null || true)
[ -n "$SUMMARY" ] && [ -e "$SUMMARY" ] || stopped "まとめ（summary.md）の場所が分かりません。" "$LOG/summary-path"
RESULT="$LOG/fix-result.md"
CHECK="$LOG/check-result.md"
DRAFT="$LOG/report-draft.md"
rm -f "$RESULT" "$CHECK" "$DRAFT"
git worktree remove --force "$FIX" 2>/dev/null || true
git worktree remove --force "$BASEW" 2>/dev/null || true
git worktree prune
git worktree add -q -B "auto-fix-$C" "$FIX" "$FULL"
git worktree add -q --detach "$BASEW" "$FULL"

common="作業場所は $FIX、前の作業場所は $BASEW、対象コミットは $FULL、まとめは $SUMMARY、手紙は $LETTER、返事は $REPLY です。人のいない所で動いているので、利用者に質問しないでください。"

note "返事を読みました。コミット $C から直し始めます"
(cd "$FIX" && caffeinate -i claude -p "$REPO/review-loop/fix-prompt.md を読み、そのとおりに直してください。$common結果は $RESULT に書いてください。" \
    --model opus --dangerously-skip-permissions) >"$LOG/fix.log" 2>&1 || true
[ -s "$RESULT" ] || stopped "直す段で、結果（fix-result.md）ができませんでした。" "$LOG/fix.log"

COMMITS=$(git -C "$FIX" rev-list --count "$FULL..HEAD")
if [ "$COMMITS" -gt 0 ]; then
    note "直したコミット $COMMITS 個を、Gemini が審査します"
    # shellcheck disable=SC2086
    (cd "$FIX" && caffeinate -i agy -p "$REPO/review-loop/check-prompt.md を読み、そのとおりに審査してください。$common直した AI の結果は $RESULT です。審査の結果は $CHECK に書いてください。" \
        ${AGY_MODEL:+--model "$AGY_MODEL"} --dangerously-skip-permissions --print-timeout "${AGY_TIMEOUT:-1440m}") \
        >"$LOG/check.log" 2>&1 || true
    [ -s "$CHECK" ] || stopped "審査の段で、審査の結果（check-result.md）ができませんでした。" "$LOG/check.log"
else
    printf '（直したコミットが無いので、審査はしていません）\n' >"$CHECK"
fi

note "審査で落とした直しを戻して、報告を書きます"
(cd "$FIX" && caffeinate -i claude -p "$REPO/review-loop/finalize-prompt.md を読み、そのとおりに進めてください。$common直した AI の結果は $RESULT、審査の結果は $CHECK です。報告は $DRAFT に書いてください。" \
    --model opus --dangerously-skip-permissions) >"$LOG/finalize.log" 2>&1 || true
[ -s "$DRAFT" ] || stopped "仕上げの段で、報告ができませんでした。" "$LOG/finalize.log"

# AI の言うことによらない、最後の確かめ
[ -z "$(git -C "$FIX" status --porcelain)" ] || stopped "直す段の作業場所に、コミットしていない変更が残っています。プッシュしていません。" "$FIX"
if [ "$(git -C "$FIX" rev-list --count "$FULL..HEAD")" -gt 0 ]; then
    note "整形・全テスト・clippy をもう一度確かめます"
    (cd "$FIX" && cargo fmt --check && cargo test --release && cargo clippy --release --all-targets -- -D warnings) \
        >"$LOG/gate.log" 2>&1 || stopped "最後の確かめ（整形・全テスト・clippy）が通らなかったので、プッシュしていません。報告の下書き：$DRAFT" "$LOG/gate.log"
    git -C "$FIX" push -q "$REMOTE" "HEAD:refs/heads/$BRANCH" >"$LOG/push.log" 2>&1 \
        || stopped "プッシュできませんでした（ほかの変更が先に入った、など）。報告の下書き：$DRAFT" "$LOG/push.log"
    PUSHED="プッシュしました（$(git -C "$FIX" rev-parse --short=7 HEAD)）。次の見回りで、このコミットをまたレビューします。"
else
    PUSHED="直したコミットが残らなかったので、プッシュしていません。"
fi
{
    cat "$DRAFT"
    printf '\n---\n%s\n' "$PUSHED"
} >"$REPORT"
git worktree remove --force "$FIX" 2>/dev/null || true
git worktree remove --force "$BASEW" 2>/dev/null || true
git branch -D "auto-fix-$C" >/dev/null 2>&1 || true
note "報告が届きました：$REPORT"

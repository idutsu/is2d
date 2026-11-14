#!/bin/sh
# is2d のレビューと修正の繰り返しの「見回り」1回分。launchd が 10 分ごとに動かす（install.sh で登録する）。
# レビューも直す作業も、このリポジトリのフォルダで行う。見回りのたびに、手元を GitHub の最新に追いつかせる
# （git pull --ff-only）。今どの段かは claude/review-loop/ のファイルの有る無しで決め、やることがあるときだけ進める。
#   <C> は、追いつかせたあとの手元のコミット（7桁）
#
#   STOP がある                       → 何もしない
#   止まった.md がある                → 何もしない（中の理由を見て、そのファイルを消すと、次の見回りでやり直す）
#   保存していない変更がある          → 何もしない（あなたの作業を消さないため。変更が無くなったら続ける）
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
STOPPED="$DIR/止まった.md"
DIRTY="$DIR/保存していない変更があります.md"
[ -e "$DIR/STOP" ] && exit 0
[ -e "$STOPPED" ] && exit 0

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

C=unknown
LOG="$REPO/claude/review-logs"
# 止まった理由を書いて、この見回りを終える（同じ所で何度もやり直さないため。ファイルを消すとやり直す）
stopped() {
    {
        printf '# 止まりました（コミット %s）\n\n' "$C"
        printf '%s\n\n' "$1"
        printf -- '- 記録：%s\n\n' "$2"
        printf 'このファイルを消すと、次の見回り（10 分以内）でやり直します。\n'
    } >"$STOPPED"
    note "止まりました：$1"
    exit 0
}

# あなたの作業を消さない：保存していない変更（Git に入らないフォルダの中を除く）があれば、何もしない
if [ -n "$(git status --porcelain)" ]; then
    if [ ! -e "$DIRTY" ]; then
        {
            printf '# 保存していない変更があるので、待っています\n\n'
            printf 'このフォルダ（%s）に、コミットしていない変更があります。見回りは、変更が無くなるまで何もしません。\n\n' "$REPO"
            printf '```\n%s\n```\n' "$(git status --short)"
        } >"$DIRTY"
        note "保存していない変更があるので、待っています（$DIRTY）"
    fi
    exit 0
fi
rm -f "$DIRTY"

git pull -q --ff-only >"$DIR/pull.log" 2>&1 || stopped "手元を GitHub の最新に追いつかせられませんでした（手元に、GitHub に無いコミットがある、など）。" "$DIR/pull.log"
UP=$(git rev-parse --abbrev-ref '@{upstream}')
REMOTE=${UP%%/*}
BRANCH=${UP#*/}
BASE=$(git rev-parse HEAD)
C=$(git rev-parse --short=7 HEAD)
LETTER="$DIR/$C-手紙.md"
REPLY="$DIR/$C-返事.md"
REPORT="$DIR/$C-報告.md"
LOG="$REPO/claude/review-logs/$C"
[ -e "$REPORT" ] && exit 0

# AI が残したもの（コミット・コミットしていない変更）を控えのブランチ auto-<段>-<C> に移し、フォルダをこの見回りの
# 始め（BASE）に戻す。見回りの始めにはフォルダに変更が無かったので、残っているのは AI が作ったものだけ。
# 移したときはその説明を、何も無ければ空を出す
restore_folder() {
    if [ "$(git rev-parse HEAD)" = "$BASE" ] && [ -z "$(git status --porcelain)" ]; then
        return 0
    fi
    git add -A >/dev/null 2>&1 || true
    git -c user.name=is2d-loop -c user.email=is2d-loop@localhost commit -q --no-verify \
        -m "見回りが残したもの（コミットしていなかった変更）" >/dev/null 2>&1 || true
    git branch -f "auto-$1-$C" HEAD >/dev/null 2>&1 || true
    git reset -q --hard "$BASE"
    git clean -q -fd
    printf '%s' "残っていたもの（コミット・変更）は、ブランチ auto-$1-$C に移し、フォルダは元に戻しました。"
}

if [ ! -e "$LETTER" ]; then
    note "コミット $C をレビューします（Claude と Gemini が1つずつ。何時間もかかります）"
    if IS2D_LETTER="$LETTER" IS2D_REPLY="$REPLY" caffeinate -i sh "$REPO/review-all.sh" >>"$DIR/review-all.out" 2>&1; then
        moved=$(restore_folder review)
        note "手紙が届きました：$LETTER $moved"
    else
        moved=$(restore_folder review)
        stopped "レビューかまとめが途中で止まりました。$moved" "$LOG/progress.log"
    fi
    exit 0
fi

[ -e "$REPLY" ] || exit 0

# ---- 返事があるので直す（このフォルダで。止まるときは、直しかけを控えのブランチに残して元に戻す）----
mkdir -p "$LOG"
SUMMARY=$(cat "$LOG/summary-path" 2>/dev/null || true)
[ -n "$SUMMARY" ] && [ -e "$SUMMARY" ] || stopped "まとめ（summary.md）の場所が分かりません。" "$LOG/summary-path"
RESULT="$LOG/fix-result.md"
CHECK="$LOG/check-result.md"
DRAFT="$LOG/report-draft.md"
rm -f "$RESULT" "$CHECK" "$DRAFT"

# 直す段で止まるとき：直しかけを控えのブランチ auto-fix-<C> に移し、フォルダを直す前（BASE）に戻してから止まる
stop_fixing() {
    moved=$(restore_folder fix)
    stopped "$1 $moved 報告の下書き（あれば）：$DRAFT" "$2"
}

common="作業場所は $REPO、対象コミットは $BASE、まとめは $SUMMARY、手紙は $LETTER、返事は $REPLY です。人のいない所で動いているので、利用者に質問しないでください。"

note "返事を読みました。コミット $C から直し始めます"
caffeinate -i claude -p "$REPO/review-loop/fix-prompt.md を読み、そのとおりに直してください。$common結果は $RESULT に書いてください。" \
    --model opus --dangerously-skip-permissions >"$LOG/fix.log" 2>&1 || true
[ -s "$RESULT" ] || stop_fixing "直す段で、結果（fix-result.md）ができませんでした。" "$LOG/fix.log"

COMMITS=$(git rev-list --count "$BASE..HEAD")
if [ "$COMMITS" -gt 0 ]; then
    note "直したコミット $COMMITS 個を、Gemini が審査します"
    # shellcheck disable=SC2086
    caffeinate -i agy -p "$REPO/review-loop/check-prompt.md を読み、そのとおりに審査してください。$common直した AI の結果は $RESULT です。審査の結果は $CHECK に書いてください。" \
        ${AGY_MODEL:+--model "$AGY_MODEL"} --dangerously-skip-permissions --print-timeout "${AGY_TIMEOUT:-1440m}" \
        >"$LOG/check.log" 2>&1 || true
    [ -s "$CHECK" ] || stop_fixing "審査の段で、審査の結果（check-result.md）ができませんでした。" "$LOG/check.log"
else
    printf '（直したコミットが無いので、審査はしていません）\n' >"$CHECK"
fi

note "審査で落とした直しを戻して、報告を書きます"
caffeinate -i claude -p "$REPO/review-loop/finalize-prompt.md を読み、そのとおりに進めてください。$common直した AI の結果は $RESULT、審査の結果は $CHECK です。報告は $DRAFT に書いてください。" \
    --model opus --dangerously-skip-permissions >"$LOG/finalize.log" 2>&1 || true
[ -s "$DRAFT" ] || stop_fixing "仕上げの段で、報告ができませんでした。" "$LOG/finalize.log"

# AI の言うことによらない、最後の確かめ
[ -z "$(git status --porcelain)" ] || stop_fixing "コミットしていない変更が残っていたので、プッシュしていません。" "$REPO"
if [ "$(git rev-list --count "$BASE..HEAD")" -gt 0 ]; then
    note "整形・全テスト・clippy をもう一度確かめます"
    (cargo fmt --check && cargo test --release && cargo clippy --release --all-targets -- -D warnings) \
        >"$LOG/gate.log" 2>&1 || stop_fixing "最後の確かめ（整形・全テスト・clippy）が通らなかったので、プッシュしていません。" "$LOG/gate.log"
    git push -q "$REMOTE" "HEAD:refs/heads/$BRANCH" >"$LOG/push.log" 2>&1 \
        || stop_fixing "プッシュできませんでした（ほかの変更が先に入った、など）。" "$LOG/push.log"
    PUSHED="プッシュしました（$(git rev-parse --short=7 HEAD)）。次の見回りで、このコミットをまたレビューします。"
else
    PUSHED="直したコミットが残らなかったので、プッシュしていません。"
fi
{
    cat "$DRAFT"
    printf '\n---\n%s\n' "$PUSHED"
} >"$REPORT"
note "報告が届きました：$REPORT"

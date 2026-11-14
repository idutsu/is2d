#!/bin/sh
# 10 分ごとの見回り（loop.sh）を、この Mac の launchd に登録する。止めるときは uninstall.sh。
#
# 使い方：リポジトリの中で、claude・agy・cargo・git が使えるターミナルから
#   AGY_MODEL=<agy models で確かめた Carbon の名前> sh review-loop/install.sh
# 今のターミナルの PATH を、そのまま見回りに渡す（launchd は PATH をほとんど持たないため）。
set -eu

REPO=$(cd "$(dirname "$0")/.." && pwd)
LABEL=com.is2d.review-loop
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
mkdir -p "$HOME/Library/LaunchAgents" "$REPO/claude/review-loop"

for cmd in git claude agy cargo caffeinate; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        echo "$cmd が見つかりません。PATH に入れてから、もう一度流してください。" >&2
        exit 1
    fi
done
git -C "$REPO" rev-parse '@{upstream}' >/dev/null 2>&1 || {
    echo "今のブランチが GitHub のブランチを追いかけていません（@{upstream} がありません）。" >&2
    exit 1
}

# plist に書く文字（& < > を XML の書き方にする）
xml() {
    printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'
}

cat >"$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$LABEL</string>
    <key>ProgramArguments</key>
    <array>
        <string>/bin/sh</string>
        <string>$(xml "$REPO/review-loop/loop.sh")</string>
    </array>
    <key>StartInterval</key>
    <integer>600</integer>
    <key>RunAtLoad</key>
    <true/>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>$(xml "$PATH")</string>
        <key>AGY_MODEL</key>
        <string>$(xml "${AGY_MODEL:-}")</string>
    </dict>
    <key>StandardOutPath</key>
    <string>$(xml "$REPO/claude/review-loop/launchd.log")</string>
    <key>StandardErrorPath</key>
    <string>$(xml "$REPO/claude/review-loop/launchd.log")</string>
</dict>
</plist>
EOF

launchctl bootout "gui/$(id -u)/$LABEL" >/dev/null 2>&1 || true
launchctl bootstrap "gui/$(id -u)" "$PLIST"
echo "登録しました。10 分ごとに見回ります（すぐに1回目が動きます）。"
echo "  手紙・返事・報告：$REPO/claude/review-loop/"
echo "  進み具合：$REPO/claude/review-loop/progress.log"
echo "  一時的に止める：$REPO/claude/review-loop/STOP というファイルを置く"
echo "  登録を外す：sh review-loop/uninstall.sh"

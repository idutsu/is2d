#!/bin/sh
# 10 分ごとの見回り（install.sh で登録したもの）を、launchd から外す。今動いている見回りは、そのまま最後まで動く。
set -eu
LABEL=com.is2d.review-loop
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
launchctl bootout "gui/$(id -u)/$LABEL" >/dev/null 2>&1 || true
rm -f "$PLIST"
echo "登録を外しました。手紙・返事・報告のファイルはそのまま残しています。"

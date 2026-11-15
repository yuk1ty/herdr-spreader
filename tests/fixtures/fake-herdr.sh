#!/bin/sh
# fake-herdr.sh
#
# A canned stand-in for the real `herdr` CLI, used by the Step 21 / Unit 3B
# integration test to exercise CliBackend end-to-end without a running herdr
# server.
#
# Behavior:
#   - Appends the full argv it was invoked with (space-joined, one line) to
#     the file named by $FAKE_HERDR_LOG, if that env var is set.
#   - Based on the first argv token (the herdr subcommand), echoes canned
#     JSON matching the real herdr response shapes documented in
#     `.claude/user/plan/task-graph.md` section "3. 共有コンテキスト".
#   - Always exits 0 (this fixture is for the happy path only).

if [ -n "$FAKE_HERDR_LOG" ]; then
  echo "$@" >> "$FAKE_HERDR_LOG"
fi

case "$1" in
  workspace)
    case "$2" in
      list)
        # Seed via $FAKE_HERDR_WORKSPACES: comma-separated labels. Ids are
        # generated as x0, x1, ... in order.
        items=""
        n=0
        IFS=','
        for label in ${FAKE_HERDR_WORKSPACES:-}; do
          if [ -n "$label" ]; then
            if [ -n "$items" ]; then items="$items,"; fi
            items="$items{\"workspace_id\":\"x$n\",\"label\":\"$label\"}"
          fi
          n=$((n + 1))
        done
        unset IFS
        cat <<JSON
{"id":"cli:workspace:list","result":{"type":"workspace_list","workspaces":[$items]}}
JSON
        ;;
      focus)
        # workspace focus discards stdout; nothing to emit.
        ;;
      create)
        # Argv logging above runs before this branch, so by the Nth
        # `workspace create` call the log already contains N matching lines:
        # the 1st call sees count=1 (-> wA), the 2nd sees count=2 (-> wB).
        count=$(grep -c '^workspace create' "$FAKE_HERDR_LOG" 2>/dev/null || echo 1)
        if [ "$count" -ge 2 ]; then
          ws=wB
        else
          ws=wA
        fi
        cat <<JSON
{"result":{"type":"workspace_created","workspace":{"workspace_id":"$ws","label":"demo","active_tab_id":"$ws:t1"},"tab":{"tab_id":"$ws:t1","label":"1"},"root_pane":{"pane_id":"$ws:p1","tab_id":"$ws:t1","workspace_id":"$ws","cwd":"/tmp"}}}
JSON
        ;;
    esac
    ;;
  tab)
    case "$2" in
      create)
        cat <<'JSON'
{"result":{"type":"tab_created","tab":{"tab_id":"wA:t2"},"root_pane":{"pane_id":"wA:p2"}}}
JSON
        ;;
      rename)
        # rename_tab discards stdout; nothing to emit.
        ;;
    esac
    ;;
  pane)
    case "$2" in
      split)
        cat <<'JSON'
{"result":{"type":"pane_info","pane":{"pane_id":"wA:p3","tab_id":"wA:t1"}}}
JSON
        ;;
      run|focus)
        # run/focus discard stdout; nothing to emit.
        ;;
    esac
    ;;
  wait)
    # wait_output discards stdout; nothing to emit.
    ;;
esac

exit 0

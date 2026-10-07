#!/usr/bin/env bash
# Run the preference-source, transition, palette and window-decoration tests
# on each supported host OS. Native desktop settings are covered separately.
set -euo pipefail

test_theme() {
  if [[ "${GAZE_THEME_OS_KEYRING:-0}" == 1 ]]; then
    cargo test --locked --release -p gaze-shell --features gaze-shell/os-keyring --lib "$@"
  else
    cargo test --locked --release -p gaze-shell --lib "$@"
  fi
}

test_theme system_theme::tests::
test_theme theme::tests::a_theme_that_cannot_be_used_falls_back_and_says_why
test_theme application::tests::the_title_bar_follows_the_system_only_where_the_os_reports_changes
test_theme application::tests::a_scheme_request_changes_only_what_differs

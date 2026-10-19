#!/usr/bin/env bash
# Run the preference-source, displayed palette and window-decoration tests
# on each supported host OS and architecture.
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
test_theme application::tests::only_theme_changes_report_a_scheme
test_theme application::tests::the_title_bar_follows_the_system_only_where_the_os_reports_changes
test_theme application::tests::a_scheme_request_changes_only_what_differs
test_theme chrome::tests::system_follows_the_os
test_theme chrome::tests::an_os_change_leaves_no_stale_label_colours
test_theme chrome::tests::an_explicit_choice_ignores_the_os
test_theme chrome::tests::no_preference_means_dark
test_theme chrome::tests::the_override_ignores_window_reports
test_theme chrome::tests::a_portal_answer_applies_on_the_next_poll
test_theme chrome::tests::the_window_is_asked_to_show_the_scheme
test_theme chrome::tests::choosing_system_asks_the_system_again
test_theme chrome::tests::pages_follow_the_chrome_scheme

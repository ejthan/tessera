# Security checklist

One row per point, with its proof: a test, a CI job or a file. Status is **Proved** when that proof runs in `cargo test --workspace` or CI, **Partly** when a named part is not covered, and **Manual** when only a person can check it. A test is written `crate/tests/file.rs::name`, or `src/file.rs::name` for a unit test. Filled in at the end of M8, spec §9 and D85–D99. A proof in `live_*` runs only against a real server, and those did not run on the last commit of M8: see step 16 in `docs/building.md`.

## Spec §9

| Point | Status | Proof |
|---|---|---|
| Keys only in the OS keychain | Proved | `crates/tessera-core/src/secrets.rs::file_never_contains_the_key`, `crates/tessera-app/tests/ui_import.rs::export_writes_profiles_without_keys` |
| Secrets in memory as `SecretString`, zeroized on drop, with the exceptions below | Partly | `crates/tessera-client/tests/network.rs::no_key_is_copied_into_a_plain_string`. It stops a new plain copy; that memory is really wiped is `secrecy` and `zeroize` doing their work, which no test here observes |
| `http://` to a remote host and insecure certificates show a banner on every tab | Proved | `crates/tessera-app/tests/ui_safety.rs::remote_http_shows_the_banner`, `::insecure_certificates_show_the_banner`, `::https_and_local_show_no_banner` |
| The log masks keys and secret fields on stderr and in the file | Proved | `crates/tessera-app/tests/logging.rs::a_real_session_logs_no_key_and_no_body`, `::a_file_has_the_prefix_and_no_secret`, `::the_debug_line_of_send_never_carries_a_key`, `crates/tessera-core/src/redact.rs::an_unterminated_quote_masks_to_the_end` |
| Default log level `info` | Proved | `crates/tessera-app/src/logging.rs` (`init`), `crates/tessera-app/tests/logging.rs::a_real_session_logs_no_key_and_no_body`, which runs at `trace`, the worst case |
| Request bodies reach only the in-memory log | Proved | `crates/tessera-app/tests/logging.rs::a_real_session_logs_no_key_and_no_body` |
| Log files private to the user | Proved | `crates/tessera-core/src/store.rs::a_private_dir_is_this_users_alone`. On Windows the directory's own ACL applies, which is not tested |
| Backups private to the user | Proved | `crates/tessera-core/tests/backup_run.rs::a_folder_backup_is_private`, `crates/tessera-core/tests/backup_archive.rs::what_is_written_is_private` |
| A backup holds no provider key | Proved | `crates/tessera-core/tests/backup_run.rs::a_backup_holds_no_provider_key` |
| `docs/security-checklist.md` names each proof | Proved | This file |
| No telemetry; connections only to configured nodes and, opt-in, GitHub | Proved | `crates/tessera-client/tests/network.rs::only_two_places_build_an_http_client`, `crates/tessera-client/tests/update.rs::sends_a_user_agent_and_no_key`, `::follows_no_redirect`, `crates/tessera-app/tests/ui_settings.rs::the_update_check_is_off_and_sends_nothing`, `::turning_the_check_on_sends_nothing_until_a_start` |
| `cargo deny` in CI | Proved | Job `lint` in `.github/workflows/ci.yml`, `deny.toml`. Locally on 2026-11-07: advisories, bans, licenses and sources ok |
| `SECURITY.md` with the process and 7 days | Proved | `SECURITY.md` |

## Zeroization: the exceptions of design §8.2

| Place | Status | Why it stands |
|---|---|---|
| `HeaderValue` per request | Accepted | Marked sensitive, held by `reqwest`, freed after the request. Not zeroized |
| `InputState` of a masked input | Accepted | A rope in the kit, which has no hook. Zeroized text is copied into it, and that copy stays until the input goes |
| `SharedString` on the way into an input | Accepted | `default_value` takes one, built from the zeroized `&str` with no `String` between. It is not zeroized |
| `ClipboardItem` and the clipboard | Accepted | Copy is the user's own act. The item takes a `String`, the one plain copy in `key_dialog.rs`, and the system holds the value after it |
| The keychain's own buffers | Accepted | Inside `keyring` and the platform |

## Spec §7.1, acceptance

| Point | Status | Proof |
|---|---|---|
| A key never shows in logs | Proved | See §9 above |
| A key never shows in config files | Proved | `crates/tessera-core/src/secrets.rs::file_never_contains_the_key`, `crates/tessera-app/tests/ui_search.rs::history_never_holds_a_key`, `::save_needs_a_name_and_keeps_no_key` |
| A key never shows in crash reports | Partly | Tessera writes none. A panic prints to stderr, unmasked by the log writer, and no test provokes one |
| A read-only profile sends no write from any UI path | Proved | `crates/tessera-client/tests/client.rs::read_only_blocks_writes_before_sending`, the client's hook under every path, and `crates/tessera-app/tests/ui_unlock.rs::write_on_read_only_opens_unlock_and_sends_nothing` |

## Spec §7.19, redaction

| Point | Status | Proof |
|---|---|---|
| Keys masked in the request log, its detail and its full body | Proved | `crates/tessera-app/tests/ui_log.rs::full_secret_session_scan`, `::query_key_is_masked_in_row`, `::detail_shows_redacted_body`, `::show_full_body_opens_the_view_masked` |

## Spec §7.14, the one-time value

| Point | Status | Proof |
|---|---|---|
| Shown once, masked until asked, and Esc does not lose it | Proved | `crates/tessera-app/tests/ui_keys.rs::reveal_shows_masked_and_copy_gives_the_value`, `::reveal_ignores_escape`, `::reveal_works_from_the_keyboard`, `::done_closes_the_reveal_and_selects_the_key` |

## Spec §7.16, provider keys

| Point | Status | Proof |
|---|---|---|
| A provider key reaches no log, trace or file | Proved | `crates/tessera-app/tests/ui_log.rs::provider_key_session_scan` |
| The stored key is never shown, and a mask is never stored | Proved | `crates/tessera-app/tests/ui_ai.rs::stored_mask_is_never_shown`, `::typed_mask_is_refused_inline`, `::save_without_a_key_sends_no_api_key`, `::selection_change_clears_the_key` |

## Read-only guard

| Point | Status | Proof |
|---|---|---|
| The client refuses before sending | Proved | `crates/tessera-client/tests/client.rs::read_only_blocks_writes_before_sending` |
| A write unlocks first, and the name is typed | Proved | `crates/tessera-app/tests/ui_unlock.rs::write_on_read_only_opens_unlock_and_sends_nothing`, `crates/tessera-app/tests/ui_backup.rs::read_only_unlocks_first_and_prod_is_typed` |
| A disconnect locks a running job | Proved | `crates/tessera-app/tests/ui_jobs.rs::disconnect_stops_a_write_job_of_an_unlocked_read_only_profile`, `crates/tessera-app/tests/ui_backup.rs::a_disconnect_during_a_restore_is_named` |

## Danger table

Typed means the name is typed, and default focus is on Cancel: `crates/tessera-app/tests/ui_aliases.rs::focus_starts_on_cancel_in_confirm`, and the audit of red buttons in `crates/tessera-app/tests/ui_safety.rs`.

| Decision | What | Status | Proof |
|---|---|---|---|
| D28 | Document deletes and bulk updates: typed above the threshold or on prod | Proved | `crates/tessera-core/src/documents/export.rs::danger_levels`, `crates/tessera-app/tests/ui_documents.rs::confirm_above_of_5_makes_a_delete_of_6_typed`, `::confirm_above_of_5_leaves_a_delete_of_5_plain` |
| D40 | The raw console | Proved | `crates/tessera-core/src/raw.rs::raw_danger_table`, `crates/tessera-app/tests/ui_console.rs::prod_post_asks` |
| D49 | Imports | Proved | `crates/tessera-app/tests/ui_import_documents.rs::update_on_prod_types_the_name`, `::create_on_prod_confirms_with_focus_on_cancel` |
| D62 | M6 resources and the reindex switch | Proved | `crates/tessera-app/tests/ui_aliases.rs::repoint_on_prod_needs_the_name`, `crates/tessera-app/tests/ui_reindex.rs::prod_switch_is_typed`, `crates/tessera-app/tests/ui_sets.rs::json_removal_on_prod_needs_the_name` |
| D81 | M7 keys, rules, models and dictionaries | Proved | `crates/tessera-app/tests/ui_keys.rs::delete_on_prod_is_typed`, `crates/tessera-app/tests/ui_ai.rs::prod_delete_is_typed`, `crates/tessera-app/tests/ui_stemming.rs::delete_is_typed_when_used`, `crates/tessera-app/tests/ui_analytics.rs::test_event_on_prod_asks_first` |
| D85 | Operations | Proved | `crates/tessera-app/tests/ui_operations.rs::snapshot_and_vote_are_typed_on_prod`, `::compact_stays_plain_on_prod_and_config_asks_there` |

## Backups

| Point | Status | Proof |
|---|---|---|
| An unpack refuses traversal, links, devices and long-name entries | Proved | `crates/tessera-core/tests/backup_archive.rs::traversal_is_refused`, `::entries_that_are_no_regular_files_are_refused`, `::a_long_name_entry_cannot_rename_a_file` |
| An unpack stops at the declared size | Proved | `crates/tessera-core/tests/backup_archive.rs::more_bytes_than_declared_stop_at_the_declared_size` |
| Every file is checked before a restore | Proved | `crates/tessera-core/tests/backup_archive.rs::verify_finds_a_flipped_byte`, `::verify_refuses_a_link_in_a_folder` |
| A restore never writes to an existing collection, and reads the server again first | Proved | `crates/tessera-core/tests/backup_run.rs::a_refresh_that_finds_a_new_collection_writes_nothing`, `::a_conflict_on_create_imports_nothing`, `::documents_are_created_never_upserted` |
| A blanked key blocks the collection | Proved | `crates/tessera-core/tests/backup_run.rs::a_restore_of_blanked_keys_says_so_and_blocks_the_collection` |
| Only rows approved as Differs are replaced | Proved | `crates/tessera-core/tests/backup_run.rs::a_row_that_became_different_after_the_confirmation_stays` |
| Clear cache leaves what is in use | Proved | `crates/tessera-app/tests/ui_backup.rs::clear_cache_keeps_a_held_directory_and_reloads_the_lists`, `crates/tessera-app/tests/ui_backup.rs::an_archive_is_held_while_it_unpacks`, `crates/tessera-app/src/relevance.rs::only_old_copies_of_this_user_are_swept` |
| A real backup restored into a real server, with a real provider | Manual | The round trip is `crates/tessera-core/tests/live_backup.rs`, not run on the last commit. A real provider and the checks on screen are in `docs/building.md` |

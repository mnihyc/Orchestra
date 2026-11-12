# Version and update controls

Build selection, checksum admission, native checking and reviewed update use
the existing program/catalogue/tool-cache mechanisms. The corpus loader adds no
version resolver, independent cache or automatic update. Program kind is data;
the shared selector tests include Codex, Claude Code, dsh and a fourth kind.

| Rule | Deciding native control |
|---|---|
| Shipped / Newest checked / Newest / Exact | API `hosting::upgrade_tests::every_program_resolves_shipped_checked_newest_and_exact_without_retargeting_a_recorded_pin` |
| Same selection across all three programs and a fourth | That same table, actual requested_of selector; no kind branch |
| Checksum mismatch always refused | API `hosting::upgrade_tests::checksum_refusal_remains_a_refusal_for_every_program_and_policy`; Node `tools::what_arrives_with_another_hash_than_the_system_named_is_thrown_away` |
| Untested is visible, not an invented checked result | Fake `harness::a_harness_version_below_the_range_is_refused_and_one_above_it_is_run_untested`; API `harness_checks::a_kept_build_is_checked_on_a_host_of_its_system_and_keeps_what_the_check_found` |
| Existing unit keeps its recorded build until review | Engine `updates::a_profile_pin_is_provisioned_updated_and_unavailable_pins_block_without_commands`, real runtime/preflight/command path |
| Explicit downgrade and unavailable pin | The same Engine control, plus shared API policy table |
| Rollback after failed/cancelled replacement | Engine `updates::a_failed_replacement_restores_the_old_unit_generation_files_and_pending_inputs`, `cancelling_a_replacement_before_commit_restores_the_old_unit` |
| Mixed versions across independent hosts; one-unit conflict | API `hosting::upgrade_tests::mixed_machine_versions_are_independent_while_one_unit_cannot_satisfy_conflicting_pins`; Node cache keeps version-named immutable directories |
| Cache entry cannot be silently overwritten | Node `tools::an_existing_cache_entry_is_not_relabelled_or_replaced`, `a_version_in_the_cache_is_never_replaced_not_even_by_a_build_with_another_hash` |
| First cache publication is verified/executable and checked | Node `tools::a_program_is_downloaded_verified_and_published_whole_and_executable`; API harness_checks explicitly observes `node.tool.ensure`, then `node.tool.check`, then `node.tool.conformance` |
| Exact channel fetch uses its own checksum/check | API `harness_checks::channel_cases::a_typed_exact_release_uses_its_own_manifest_checksum_and_background_native_check` |
| dsh index spelling preserves one build identity | API `hosting::upgrade_tests::a_python_index_pre_release_pin_keeps_the_canonical_existing_build_identity` |
| A fourth manifest gets native interface checks without new program code | API `harness_checks::a_program_that_is_nothing_but_a_manifest_has_its_builds_checked_by_its_interface_alone` |

These are test names, not claims that this checkout ran them. The integrator
compiles and runs the named native suites. Live vendor smoke remains independent
of schema-derived or scripted offline evidence.

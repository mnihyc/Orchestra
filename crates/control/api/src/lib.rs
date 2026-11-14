//! Stable API assembly and compatibility paths.
#![forbid(unsafe_code)]
#![cfg_attr(
    any(test, feature = "test-support"),
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "the native test cases fail by panicking"
    )
)]
#[cfg(feature = "test-observers")]
pub use orch_api_base::test_observers;

pub mod archives {

    pub use orch_api_base::archives::READ_TIME;

    pub use orch_api_base::archives::ZIP_ENTRIES_MAX;

    pub use orch_api_base::archives::ArchiveError;

    pub use orch_api_base::archives::preview_count;

    pub use orch_api_base::archives::list;

    pub use orch_api_base::archives::DocumentSource;

    pub use orch_api_base::archives::documents;

    pub use orch_api_base::archives::read_entry;
    #[cfg(test)]
    mod tests {
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread")]
        async fn one_entry_of_a_large_zip_is_read_by_asking_the_store_for_a_small_part_of_it() {
            orch_api_base::archives::tests::__api_run_one_entry_of_a_large_zip_is_read_by_asking_the_store_for_a_small_part_of_it().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread")]
        async fn a_tar_is_found_in_every_compression_by_its_first_bytes_and_not_by_its_name() {
            orch_api_base::archives::tests::__api_run_a_tar_is_found_in_every_compression_by_its_first_bytes_and_not_by_its_name().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread")]
        async fn a_file_that_is_no_archive_is_refused_and_so_is_a_zip_that_is_cut_short() {
            orch_api_base::archives::tests::__api_run_a_file_that_is_no_archive_is_refused_and_so_is_a_zip_that_is_cut_short().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread")]
        async fn an_entry_is_read_up_to_the_limit_and_a_name_that_is_not_a_file_is_not_read() {
            orch_api_base::archives::tests::__api_run_an_entry_is_read_up_to_the_limit_and_a_name_that_is_not_a_file_is_not_read().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread")]
        async fn a_tar_that_is_read_for_longer_than_may_be_says_so() {
            orch_api_base::archives::tests::__api_run_a_tar_that_is_read_for_longer_than_may_be_says_so().await
        }
        #[cfg(test)]
        #[test]
        fn a_zip_that_claims_more_entries_than_are_looked_into_is_known_by_its_end_record() {
            orch_api_base::archives::tests::__api_run_a_zip_that_claims_more_entries_than_are_looked_into_is_known_by_its_end_record()
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread")]
        async fn a_zip_with_a_password_is_listed_and_none_of_its_entries_is_opened() {
            orch_api_base::archives::tests::__api_run_a_zip_with_a_password_is_listed_and_none_of_its_entries_is_opened().await
        }
    }
}
#[cfg(test)]
pub(crate) mod ask {
    #[cfg(test)]
    mod addresses {
        #[cfg(test)]
        #[allow(
            clippy::unwrap_used,
            clippy::expect_used,
            reason = "a deciding case fails by panicking"
        )]
        mod tests {
            #[cfg(test)]
            #[test]
            fn links_accept_only_the_current_and_declared_normalized_origins() {
                orch_api_base::ask::addresses::tests::__api_run_links_accept_only_the_current_and_declared_normalized_origins()
            }
            #[cfg(test)]
            #[test]
            fn links_reject_foreign_credentials_ports_schemes_controls_and_backslashes() {
                orch_api_base::ask::addresses::tests::__api_run_links_reject_foreign_credentials_ports_schemes_controls_and_backslashes()
            }
        }
    }
    #[cfg(test)]
    mod search {
        #[cfg(test)]
        #[allow(
            clippy::unwrap_used,
            clippy::expect_used,
            reason = "a deciding case fails by panicking"
        )]
        mod tests {
            #[cfg(test)]
            #[test]
            fn ranked_search_keeps_native_sources_and_requires_its_terminal_cursor() {
                orch_api_base::ask::search::tests::__api_run_ranked_search_keeps_native_sources_and_requires_its_terminal_cursor()
            }
        }
    }
    #[cfg(test)]
    pub(crate) mod tools {
        #[cfg(test)]
        mod documentation {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn canonical_headings_follow_front_matter_and_ignore_fenced_code()
                -> ::orch_api_base::middleware::requests::ApiResult<()> {
                    orch_api_base::ask::tools::documentation::tests::__api_run_canonical_headings_follow_front_matter_and_ignore_fenced_code()
                }
                #[cfg(test)]
                #[test]
                fn public_documentation_returns_both_languages_and_rejects_extra_arguments()
                -> ::orch_api_base::middleware::requests::ApiResult<()> {
                    orch_api_base::ask::tools::documentation::tests::__api_run_public_documentation_returns_both_languages_and_rejects_extra_arguments()
                }
            }
        }
        #[cfg(test)]
        mod parameters {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn list_filters_retain_the_native_query_and_cannot_change_the_list_kind()
                -> ::orch_api_base::middleware::requests::ApiResult<()> {
                    orch_api_base::ask::tools::parameters::tests::__api_run_list_filters_retain_the_native_query_and_cannot_change_the_list_kind()
                }
                #[cfg(test)]
                #[test]
                fn document_history_keeps_native_cursor_and_replays_its_exclusive_lower_bound()
                -> ::orch_api_base::middleware::requests::ApiResult<()> {
                    orch_api_base::ask::tools::parameters::tests::__api_run_document_history_keeps_native_cursor_and_replays_its_exclusive_lower_bound()
                }
                #[cfg(test)]
                #[test]
                fn thread_and_chat_history_keep_their_native_sequence_windows()
                -> ::orch_api_base::middleware::requests::ApiResult<()> {
                    orch_api_base::ask::tools::parameters::tests::__api_run_thread_and_chat_history_keep_their_native_sequence_windows()
                }
            }
        }
        #[cfg(test)]
        mod references {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn typed_resources_and_public_addresses_keep_real_typed_identities()
                -> ::orch_api_base::middleware::requests::ApiResult<()> {
                    orch_api_base::ask::tools::references::tests::__api_run_typed_resources_and_public_addresses_keep_real_typed_identities()
                }
                #[cfg(test)]
                #[test]
                fn public_parts_use_the_same_query_and_fragment_priority_as_reference_chips()
                -> ::orch_api_base::middleware::requests::ApiResult<()> {
                    orch_api_base::ask::tools::references::tests::__api_run_public_parts_use_the_same_query_and_fragment_priority_as_reference_chips()
                }
            }
        }
        #[cfg(test)]
        mod scope {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn a_complete_named_read_does_not_require_optional_metadata()
                -> ::orch_api_base::middleware::requests::ApiResult<()> {
                    orch_api_base::ask::tools::scope::tests::__api_run_a_complete_named_read_does_not_require_optional_metadata()
                }
                #[cfg(test)]
                #[test]
                fn an_empty_native_collection_keeps_its_real_context_and_declared_proof()
                -> ::orch_api_base::middleware::requests::ApiResult<()> {
                    orch_api_base::ask::tools::scope::tests::__api_run_an_empty_native_collection_keeps_its_real_context_and_declared_proof()
                }
                #[cfg(test)]
                #[test]
                fn optional_context_cannot_promote_an_unrelated_required_resource()
                -> ::orch_api_base::middleware::requests::ApiResult<()> {
                    orch_api_base::ask::tools::scope::tests::__api_run_optional_context_cannot_promote_an_unrelated_required_resource()
                }
            }
        }
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn canonical_sources_require_checked_native_identities_and_preserve_redaction() {
                orch_api_base::ask::tools::tests::__api_run_canonical_sources_require_checked_native_identities_and_preserve_redaction()
            }
            #[cfg(test)]
            #[test]
            fn repeated_and_json_query_fields_keep_the_native_form_encoding()
            -> ::orch_api_base::middleware::requests::ApiResult<()> {
                orch_api_base::ask::tools::tests::__api_run_repeated_and_json_query_fields_keep_the_native_form_encoding()
            }
            #[cfg(test)]
            #[test]
            fn every_reference_reader_resolves_to_a_registered_read_operation()
            -> ::orch_api_base::middleware::requests::ApiResult<()> {
                orch_api_base::ask::tools::tests::__api_run_every_reference_reader_resolves_to_a_registered_read_operation()
            }
        }
    }
}

pub mod attachments {

    pub use orch_api_base::attachments::Taken;

    pub use orch_api_base::attachments::Refused;

    pub use orch_api_base::attachments::Reference;

    pub use orch_api_base::attachments::Staged;

    pub use orch_api_base::attachments::over_a_request;

    pub use orch_api_base::attachments::take;

    pub use orch_api_base::attachments::stage;

    pub use orch_api_base::attachments::Described;

    pub use orch_api_base::attachments::describe;

    pub use orch_api_base::attachments::retain;

    pub use orch_api_base::attachments::Retention;

    pub use orch_api_base::attachments::prepare_retention;

    pub use orch_api_base::attachments::require_retention;

    pub use orch_api_base::attachments::apply_retention;

    pub use orch_api_base::attachments::require;

    pub use orch_api_base::attachments::plain_media_type;

    pub use orch_api_base::attachments::is_shown_inline;

    pub use orch_api_base::attachments::serve_kept;
    #[cfg(test)]
    mod tests {
        #[cfg(test)]
        #[test]
        fn a_claimed_type_is_kept_only_when_it_is_a_plain_type_and_subtype() {
            orch_api_base::attachments::tests::__api_run_a_claimed_type_is_kept_only_when_it_is_a_plain_type_and_subtype()
        }
        #[cfg(test)]
        #[test]
        fn only_the_kinds_a_place_shows_are_shown_and_never_active_content() {
            orch_api_base::attachments::tests::__api_run_only_the_kinds_a_place_shows_are_shown_and_never_active_content()
        }
    }
}

pub mod backplane {

    pub use orch_api_base::backplane::FIRST_CONNECTOR_VERSION;

    pub use orch_api_base::backplane::RATE_WEIGHT;

    pub use orch_api_base::backplane::MAX_BACKPLANE_FRAME_BYTES;

    pub use orch_api_base::backplane::ApplyError;

    pub use orch_api_base::backplane::EventApplier;

    pub use orch_api_base::backplane::RecordsCursorsOnly;

    pub use orch_api_base::backplane::Report;

    pub use orch_api_base::backplane::LinkSettings;

    pub use orch_api_base::backplane::EntryError;

    pub use orch_api_base::backplane::Link;

    pub use orch_api_base::backplane::LinkParts;
    #[cfg(test)]
    mod tunnel_routing {
        #[cfg(test)]
        mod placement_tests {
            #[cfg(test)]
            #[test]
            fn both_sides_choose_the_lowest_sum_and_ties_prefer_the_executor() {
                orch_api_base::backplane::tunnel_routing::placement_tests::__api_run_both_sides_choose_the_lowest_sum_and_ties_prefer_the_executor()
            }
            #[cfg(test)]
            #[test]
            fn a_missing_measurement_is_not_a_zero_and_an_operator_pin_is_honoured() {
                orch_api_base::backplane::tunnel_routing::placement_tests::__api_run_a_missing_measurement_is_not_a_zero_and_an_operator_pin_is_honoured()
            }
            #[cfg(test)]
            #[test]
            fn huge_scores_do_not_overflow_and_missing_candidates_never_win() {
                orch_api_base::backplane::tunnel_routing::placement_tests::__api_run_huge_scores_do_not_overflow_and_missing_candidates_never_win()
            }
        }
    }
    // Replacement enrollment and certificate handover through session authorization.
    #[cfg(test)]
    mod component_update_tests {
        #[cfg(test)]
        mod scopes {
            #[cfg(test)]
            #[tokio::test]
            async fn update_needs_exact_scope_and_returns_native_progress_only_under_its_own_read()
            {
                orch_api_base::backplane::component_update_tests::scopes::__api_run_update_needs_exact_scope_and_returns_native_progress_only_under_its_own_read().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn actual_drain_loop_rechecks_original_scope_raw_expiry_revocation_and_physical_source()
             {
                orch_api_base::backplane::component_update_tests::scopes::__api_run_actual_drain_loop_rechecks_original_scope_raw_expiry_revocation_and_physical_source().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn a_later_controller_does_not_replace_a_closed_original_update_issuer() {
                orch_api_base::backplane::component_update_tests::scopes::__api_run_a_later_controller_does_not_replace_a_closed_original_update_issuer().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn retained_native_update_command_cannot_borrow_a_replacement_request_or_another_body()
             {
                orch_api_base::backplane::component_update_tests::scopes::__api_run_retained_native_update_command_cannot_borrow_a_replacement_request_or_another_body().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn explicit_update_rechecks_at_its_actual_physical_and_first_request_insert_waits()
             {
                orch_api_base::backplane::component_update_tests::scopes::__api_run_explicit_update_rechecks_at_its_actual_physical_and_first_request_insert_waits().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn completed_update_choice_keeps_one_request_when_response_read_closes_at_activity()
             {
                orch_api_base::backplane::component_update_tests::scopes::__api_run_completed_update_choice_keeps_one_request_when_response_read_closes_at_activity().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn minimum_version_handshake_never_lends_the_automatic_path_to_a_closed_explicit_origin()
             {
                orch_api_base::backplane::component_update_tests::scopes::__api_run_minimum_version_handshake_never_lends_the_automatic_path_to_a_closed_explicit_origin().await
            }
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_minimum_version_node_gets_one_audited_idle_grant_until_it_reconnects_on_the_recommendation()
         {
            orch_api_base::backplane::component_update_tests::__api_run_a_minimum_version_node_gets_one_audited_idle_grant_until_it_reconnects_on_the_recommendation().await
        }
    }
    #[cfg(test)]
    mod reenrollment_tests {
        #[cfg(test)]
        #[tokio::test]
        async fn a_revoked_machine_enrolls_again_without_losing_its_name_owner_or_registered_environment()
         {
            orch_api_base::backplane::reenrollment_tests::__api_run_a_revoked_machine_enrolls_again_without_losing_its_name_owner_or_registered_environment().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn rejected_certificates_release_their_version_fence_before_authorization_answers() {
            orch_api_base::backplane::reenrollment_tests::__api_run_rejected_certificates_release_their_version_fence_before_authorization_answers().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn an_active_old_certificate_is_retired_when_the_replacement_first_opens_a_session() {
            orch_api_base::backplane::reenrollment_tests::__api_run_an_active_old_certificate_is_retired_when_the_replacement_first_opens_a_session().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn an_old_identity_cannot_renew_over_the_certificate_waiting_from_reenrollment() {
            orch_api_base::backplane::reenrollment_tests::__api_run_an_old_identity_cannot_renew_over_the_certificate_waiting_from_reenrollment().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_replacement_token_cannot_identify_another_machine_and_is_good_once_and_only_in_time()
         {
            orch_api_base::backplane::reenrollment_tests::__api_run_a_replacement_token_cannot_identify_another_machine_and_is_good_once_and_only_in_time().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn retained_connector_and_node_directories_replace_only_their_bound_identity_and_keep_local_data()
         {
            orch_api_base::backplane::reenrollment_tests::__api_run_retained_connector_and_node_directories_replace_only_their_bound_identity_and_keep_local_data().await
        }
    }
    #[cfg(test)]
    mod rate_presence_tests {
        #[cfg(test)]
        #[tokio::test]
        async fn an_unmeasured_or_partial_link_stays_absent_through_report_and_machine_api() {
            orch_api_base::backplane::rate_presence_tests::__api_run_an_unmeasured_or_partial_link_stays_absent_through_report_and_machine_api().await
        }
    }
    #[cfg(test)]
    mod binding_tests {
        #[cfg(test)]
        #[tokio::test]
        async fn a_sign_in_discovered_again_on_reconnect_keeps_one_provider_and_one_profile() {
            orch_api_base::backplane::binding_tests::__api_run_a_sign_in_discovered_again_on_reconnect_keeps_one_provider_and_one_profile().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn discovery_defaults_to_the_machines_registered_home_and_preserves_a_moved_resource()
        {
            orch_api_base::backplane::binding_tests::__api_run_discovery_defaults_to_the_machines_registered_home_and_preserves_a_moved_resource().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn an_owner_without_a_personal_home_keeps_a_registration_current_and_gets_no_new_one()
        {
            orch_api_base::backplane::binding_tests::__api_run_an_owner_without_a_personal_home_keeps_a_registration_current_and_gets_no_new_one().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_profile_pointed_at_another_home_is_never_reused_for_the_default_sign_in() {
            orch_api_base::backplane::binding_tests::__api_run_a_profile_pointed_at_another_home_is_never_reused_for_the_default_sign_in().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_moved_default_login_profile_is_reused_without_resetting_its_display_home() {
            orch_api_base::backplane::binding_tests::__api_run_a_moved_default_login_profile_is_reused_without_resetting_its_display_home().await
        }
    }
    #[cfg(test)]
    mod port_records {
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn a_manager_gets_one_ordered_whole_row_page_with_both_owners_counts_and_the_earliest_measurement()
             {
                orch_api_base::backplane::port_records::tests::__api_run_a_manager_gets_one_ordered_whole_row_page_with_both_owners_counts_and_the_earliest_measurement()
            }
            #[cfg(test)]
            #[test]
            fn a_missing_carrier_inventory_is_unavailable_instead_of_fabricated_zero() {
                orch_api_base::backplane::port_records::tests::__api_run_a_missing_carrier_inventory_is_unavailable_instead_of_fabricated_zero()
            }
        }
    }
    #[cfg(test)]
    mod session_source_tests {
        #[cfg(test)]
        mod port_creation_replays {
            #[cfg(test)]
            #[tokio::test]
            async fn signed_port_use_headers_keep_tcp_and_udp_admissions_without_a_second_proof_or_native_observation()
             {
                orch_api_base::backplane::session_source_tests::port_creation_replays::__api_run_signed_port_use_headers_keep_tcp_and_udp_admissions_without_a_second_proof_or_native_observation().await
            }
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_foreign_session_report_is_acknowledged_before_ordering_without_raw_or_a_cursor_even_after_reconnect()
         {
            orch_api_base::backplane::session_source_tests::__api_run_a_foreign_session_report_is_acknowledged_before_ordering_without_raw_or_a_cursor_even_after_reconnect().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn an_unbound_segment_accepts_nobody_until_the_server_assigns_its_source() {
            orch_api_base::backplane::session_source_tests::__api_run_an_unbound_segment_accepts_nobody_until_the_server_assigns_its_source().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_foreign_session_reset_writes_no_cursor_and_sends_no_resync_while_its_source_can_reset()
         {
            orch_api_base::backplane::session_source_tests::__api_run_a_foreign_session_reset_writes_no_cursor_and_sends_no_resync_while_its_source_can_reset().await
        }
    }
    #[cfg(test)]
    mod machine_report_tests {
        #[cfg(test)]
        #[tokio::test]
        async fn an_old_segments_machine_cannot_report_usage_for_a_new_segments_turn() {
            orch_api_base::backplane::machine_report_tests::__api_run_an_old_segments_machine_cannot_report_usage_for_a_new_segments_turn().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_providers_machine_cannot_report_another_providers_lease() {
            orch_api_base::backplane::machine_report_tests::__api_run_a_providers_machine_cannot_report_another_providers_lease().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn an_environment_cannot_close_another_environments_browser_or_terminal() {
            orch_api_base::backplane::machine_report_tests::__api_run_an_environment_cannot_close_another_environments_browser_or_terminal().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn twenty_foreign_reports_are_acknowledged_counted_and_audited_once_without_content()
        {
            orch_api_base::backplane::machine_report_tests::__api_run_twenty_foreign_reports_are_acknowledged_counted_and_audited_once_without_content().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn separate_foreign_frames_keep_the_hourly_audit_append_only_and_carry_the_pending_count()
         {
            orch_api_base::backplane::machine_report_tests::__api_run_separate_foreign_frames_keep_the_hourly_audit_append_only_and_carry_the_pending_count().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn replacing_an_api_process_does_not_write_a_second_refusal_row_in_the_same_hour() {
            orch_api_base::backplane::machine_report_tests::__api_run_replacing_an_api_process_does_not_write_a_second_refusal_row_in_the_same_hour().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn foreign_environment_facts_status_and_progress_are_dropped_and_the_source_is_taken()
        {
            orch_api_base::backplane::machine_report_tests::__api_run_foreign_environment_facts_status_and_progress_are_dropped_and_the_source_is_taken().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_harness_and_provider_take_reports_only_from_their_registered_machine() {
            orch_api_base::backplane::machine_report_tests::__api_run_a_harness_and_provider_take_reports_only_from_their_registered_machine().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_managed_profile_reports_from_a_requested_colocated_harness_and_a_pinned_profile_cannot_move()
         {
            orch_api_base::backplane::machine_report_tests::__api_run_a_managed_profile_reports_from_a_requested_colocated_harness_and_a_pinned_profile_cannot_move().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_node_cannot_report_another_hosts_runtime_or_job_and_the_registered_node_can() {
            orch_api_base::backplane::machine_report_tests::__api_run_a_node_cannot_report_another_hosts_runtime_or_job_and_the_registered_node_can().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_foreign_report_in_a_node_batch_applies_none_of_the_batch() {
            orch_api_base::backplane::machine_report_tests::__api_run_a_foreign_report_in_a_node_batch_applies_none_of_the_batch().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn discarding_a_foreign_runtime_does_not_block_the_nodes_next_own_report() {
            orch_api_base::backplane::machine_report_tests::__api_run_discarding_a_foreign_runtime_does_not_block_the_nodes_next_own_report().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_foreign_environment_measurement_is_never_published_and_the_source_is_published()
        {
            orch_api_base::backplane::machine_report_tests::__api_run_a_foreign_environment_measurement_is_never_published_and_the_source_is_published().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn an_environment_stream_cannot_update_a_job_of_another_environment() {
            orch_api_base::backplane::machine_report_tests::__api_run_an_environment_stream_cannot_update_a_job_of_another_environment().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_move_destination_reports_only_requested_work_and_its_facts_wait_for_commit() {
            orch_api_base::backplane::machine_report_tests::__api_run_a_move_destination_reports_only_requested_work_and_its_facts_wait_for_commit().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_runtime_network_report_obeys_the_runtime_hosts_source_binding() {
            orch_api_base::backplane::machine_report_tests::__api_run_a_runtime_network_report_obeys_the_runtime_hosts_source_binding().await
        }
    }
    #[cfg(test)]
    pub(crate) mod command_admission_tests {
        #[cfg(test)]
        mod ask_origins {
            #[cfg(test)]
            #[tokio::test]
            async fn a_queued_ask_request_keeps_the_original_access_token_deadline_after_a_native_row_wait()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_a_queued_ask_request_keeps_the_original_access_token_deadline_after_a_native_row_wait().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn a_queued_ask_command_rechecks_its_controller_model_endpoint_and_retained_request_before_delivery()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_a_queued_ask_command_rechecks_its_controller_model_endpoint_and_retained_request_before_delivery().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn ordinary_members_cannot_change_another_persons_ask_preferences_or_delete_it()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_ordinary_members_cannot_change_another_persons_ask_preferences_or_delete_it().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn ordinary_members_cannot_read_another_persons_ask_and_event_frames_keep_current_access()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_ordinary_members_cannot_read_another_persons_ask_and_event_frames_keep_current_access().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn an_ordinary_member_cannot_stop_another_persons_ask_and_the_owner_can_stop_it_idle()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_an_ordinary_member_cannot_stop_another_persons_ask_and_the_owner_can_stop_it_idle().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn explicit_root_inspection_reads_an_ask_without_permission_to_stop_it()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_explicit_root_inspection_reads_an_ask_without_permission_to_stop_it().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn ask_creation_history_and_model_choices_require_current_access_to_the_selected_root()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_ask_creation_history_and_model_choices_require_current_access_to_the_selected_root().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn submitting_an_ask_needs_its_own_history_read_beside_message_write()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_submitting_an_ask_needs_its_own_history_read_beside_message_write().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn ordinary_members_cannot_add_or_open_another_persons_ask_attachment()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_ordinary_members_cannot_add_or_open_another_persons_ask_attachment().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn keeping_a_temporary_ask_requires_its_current_owner()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_keeping_a_temporary_ask_requires_its_current_owner().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn idle_ask_streams_read_once_then_wake_on_committed_events_and_resume_their_own_cursor()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_idle_ask_streams_read_once_then_wake_on_committed_events_and_resume_their_own_cursor().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn the_shared_thread_sweep_repairs_an_ask_event_without_a_notification()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_the_shared_thread_sweep_repairs_an_ask_event_without_a_notification().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn an_idle_ask_stream_ends_at_its_original_access_deadline_without_new_rows()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_an_idle_ask_stream_ends_at_its_original_access_deadline_without_new_rows().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn an_idle_ask_stream_wakes_on_its_own_session_revocation_without_new_rows()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_an_idle_ask_stream_wakes_on_its_own_session_revocation_without_new_rows().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn an_idle_ask_stream_rechecks_its_root_after_a_committed_permission_epoch_change()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_an_idle_ask_stream_rechecks_its_root_after_a_committed_permission_epoch_change().await
            }
        }
        #[cfg(test)]
        mod epoch_changes {
            #[cfg(test)]
            #[tokio::test]
            async fn a_changed_admission_epoch_rechecks_the_same_command_before_its_only_native_effect()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::epoch_changes::__api_run_a_changed_admission_epoch_rechecks_the_same_command_before_its_only_native_effect().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn a_second_changed_admission_epoch_refuses_without_a_native_effect()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::epoch_changes::__api_run_a_second_changed_admission_epoch_refuses_without_a_native_effect().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn a_cancelled_command_stays_cancelled_during_fresh_admission()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::epoch_changes::__api_run_a_cancelled_command_stays_cancelled_during_fresh_admission().await
            }
        }
        #[cfg(test)]
        mod http_origins {
            #[cfg(test)]
            #[tokio::test]
            async fn raw_http_queue_checks_the_original_digest_after_the_real_entry_wait()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::http_origins::__api_run_raw_http_queue_checks_the_original_digest_after_the_real_entry_wait().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn raw_http_redelivery_refuses_rotated_bearers_and_changed_physical_sources()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::http_origins::__api_run_raw_http_redelivery_refuses_rotated_bearers_and_changed_physical_sources().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn raw_http_access_deadline_survives_extension_and_is_checked_after_the_actual_wire_wait()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::http_origins::__api_run_raw_http_access_deadline_survives_extension_and_is_checked_after_the_actual_wire_wait().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn raw_http_explicit_mode_is_a_ceiling_and_identity_delivery_does_not_rerun_a_completed_source()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::http_origins::__api_run_raw_http_explicit_mode_is_a_ceiling_and_identity_delivery_does_not_rerun_a_completed_source().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn raw_http_retains_an_accepted_scope_minimum_and_keeps_it_on_the_shared_response_opening()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::http_origins::__api_run_raw_http_retains_an_accepted_scope_minimum_and_keeps_it_on_the_shared_response_opening().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn raw_http_modern_codec_rejects_missing_identity_and_a_native_caller_is_not_a_bearer()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::http_origins::__api_run_raw_http_modern_codec_rejects_missing_identity_and_a_native_caller_is_not_a_bearer().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn a_native_http_deadline_keeps_its_actual_microseconds_across_issue_wire_and_receipt()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::http_origins::__api_run_a_native_http_deadline_keeps_its_actual_microseconds_across_issue_wire_and_receipt().await
            }
        }
        #[cfg(test)]
        mod input_deliveries {
            #[cfg(test)]
            #[tokio::test]
            async fn queued_start_rechecks_its_actual_input_origin_before_first_wire()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::input_deliveries::__api_run_queued_start_rechecks_its_actual_input_origin_before_first_wire().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn queued_fenced_start_rechecks_its_actual_input_origin_before_first_wire()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::input_deliveries::__api_run_queued_fenced_start_rechecks_its_actual_input_origin_before_first_wire().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn queued_steer_rechecks_its_actual_input_origin_before_first_wire()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::input_deliveries::__api_run_queued_steer_rechecks_its_actual_input_origin_before_first_wire().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn start_cannot_publish_after_key_expiry_during_the_final_wire_cut()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::input_deliveries::__api_run_start_cannot_publish_after_key_expiry_during_the_final_wire_cut().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn fenced_start_cannot_publish_after_key_expiry_during_the_final_wire_cut()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::input_deliveries::__api_run_fenced_start_cannot_publish_after_key_expiry_during_the_final_wire_cut().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn steer_cannot_publish_after_key_expiry_during_the_final_wire_cut()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::input_deliveries::__api_run_steer_cannot_publish_after_key_expiry_during_the_final_wire_cut().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn an_actual_accepted_start_result_reconciles_after_its_origin_expires()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::input_deliveries::__api_run_an_actual_accepted_start_result_reconciles_after_its_origin_expires().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn an_actual_accepted_fenced_start_result_reconciles_after_its_origin_expires()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::input_deliveries::__api_run_an_actual_accepted_fenced_start_result_reconciles_after_its_origin_expires().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn an_actual_accepted_steer_result_reconciles_after_its_origin_expires()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::input_deliveries::__api_run_an_actual_accepted_steer_result_reconciles_after_its_origin_expires().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn a_new_scoped_steer_cannot_adopt_or_cancel_an_old_queued_none_command()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::input_deliveries::__api_run_a_new_scoped_steer_cannot_adopt_or_cancel_an_old_queued_none_command().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn a_new_scoped_steer_cannot_relabel_an_actual_accepted_none_result()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::input_deliveries::__api_run_a_new_scoped_steer_cannot_relabel_an_actual_accepted_none_result().await
            }
        }
        #[cfg(test)]
        mod mcp_imports {
            #[cfg(test)]
            #[tokio::test]
            async fn imported_mcp_keeps_its_raw_bearer_at_actual_entry_and_wire()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::mcp_imports::__api_run_imported_mcp_keeps_its_raw_bearer_at_actual_entry_and_wire().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn stored_mcp_reimport_and_offering_use_keep_the_same_raw_bearer_at_actual_entry()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::mcp_imports::__api_run_stored_mcp_reimport_and_offering_use_keep_the_same_raw_bearer_at_actual_entry().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn native_import_inverse_owns_only_the_successful_unfiled_copy_after_the_original_key_ends()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::mcp_imports::__api_run_native_import_inverse_owns_only_the_successful_unfiled_copy_after_the_original_key_ends().await
            }
        }
        #[cfg(test)]
        mod mount_locations {
            #[cfg(test)]
            #[tokio::test]
            async fn mount_ensure_records_only_its_exact_accepted_volume_and_keeps_a_write_only_receipt_after_issuer_loss()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::mount_locations::__api_run_mount_ensure_records_only_its_exact_accepted_volume_and_keeps_a_write_only_receipt_after_issuer_loss().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn mount_delete_removes_only_its_accepted_original_volume_and_completes_bookkeeping_after_issuer_loss()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::mount_locations::__api_run_mount_delete_removes_only_its_accepted_original_volume_and_completes_bookkeeping_after_issuer_loss().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn kept_purge_uses_the_exact_retained_row_and_keeps_a_current_location_on_another_machine()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::mount_locations::__api_run_kept_purge_uses_the_exact_retained_row_and_keeps_a_current_location_on_another_machine().await
            }
        }
        #[cfg(test)]
        mod provider_login {
            #[cfg(test)]
            mod provider_handover {
                #[cfg(test)]
                #[tokio::test]
                async fn queued_handover_return_and_accept_recheck_the_actual_key_before_first_reconnect_wire()
                -> ::orch_api_base::backplane::command_admission_tests::Proof {
                    orch_api_base::backplane::command_admission_tests::provider_login::provider_handover::__api_run_queued_handover_return_and_accept_recheck_the_actual_key_before_first_reconnect_wire().await
                }
                #[cfg(test)]
                #[tokio::test]
                async fn already_accepted_handover_results_remain_canonical_after_the_initiating_key_is_revoked()
                -> ::orch_api_base::backplane::command_admission_tests::Proof {
                    orch_api_base::backplane::command_admission_tests::provider_login::provider_handover::__api_run_already_accepted_handover_results_remain_canonical_after_the_initiating_key_is_revoked().await
                }
                #[cfg(test)]
                #[tokio::test]
                async fn a_succeeded_return_stays_for_its_live_custody_retry_and_is_reaped_after_release()
                -> ::orch_api_base::backplane::command_admission_tests::Proof {
                    orch_api_base::backplane::command_admission_tests::provider_login::provider_handover::__api_run_a_succeeded_return_stays_for_its_live_custody_retry_and_is_reaped_after_release().await
                }
            }
            #[cfg(test)]
            #[tokio::test]
            async fn a_queued_provider_start_rechecks_its_real_key_pending_attempt_and_setup_on_reconnect()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::provider_login::__api_run_a_queued_provider_start_rechecks_its_real_key_pending_attempt_and_setup_on_reconnect().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn a_queued_provider_input_rechecks_both_real_keys_and_the_actual_active_attempt_on_reconnect()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::provider_login::__api_run_a_queued_provider_input_rechecks_both_real_keys_and_the_actual_active_attempt_on_reconnect().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn a_queued_provider_cancel_uses_its_own_key_and_cannot_cancel_a_replacement_on_reconnect()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::provider_login::__api_run_a_queued_provider_cancel_uses_its_own_key_and_cannot_cancel_a_replacement_on_reconnect().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn a_native_login_ack_before_its_database_mirror_survives_a_later_local_authority_refusal()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::provider_login::__api_run_a_native_login_ack_before_its_database_mirror_survives_a_later_local_authority_refusal().await
            }
        }
        #[cfg(test)]
        mod publication {
            #[cfg(test)]
            #[tokio::test]
            async fn a_revoked_queued_commit_never_reaches_the_real_native_journal_on_reconnect()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::publication::__api_run_a_revoked_queued_commit_never_reaches_the_real_native_journal_on_reconnect().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn a_revoked_queued_push_never_reaches_the_real_native_journal_on_reconnect()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::publication::__api_run_a_revoked_queued_push_never_reaches_the_real_native_journal_on_reconnect().await
            }
        }
        #[cfg(test)]
        mod quota {
            #[cfg(test)]
            #[tokio::test]
            async fn ready_provider_complete_and_embed_reach_the_native_journal()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::quota::__api_run_ready_provider_complete_and_embed_reach_the_native_journal().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn cached_quota_with_credits_does_not_cancel_new_complete_admission()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::quota::__api_run_cached_quota_with_credits_does_not_cancel_new_complete_admission().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn cached_quota_with_credits_does_not_cancel_new_embed_admission()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::quota::__api_run_cached_quota_with_credits_does_not_cancel_new_embed_admission().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn cached_quota_with_credits_does_not_cancel_complete_before_transmission()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::quota::__api_run_cached_quota_with_credits_does_not_cancel_complete_before_transmission().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn cached_quota_with_credits_does_not_cancel_embed_before_transmission()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::quota::__api_run_cached_quota_with_credits_does_not_cancel_embed_before_transmission().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn explicit_provider_disable_still_blocks_complete_and_embed()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::quota::__api_run_explicit_provider_disable_still_blocks_complete_and_embed().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn explicit_model_disable_still_blocks_complete_and_embed()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::quota::__api_run_explicit_model_disable_still_blocks_complete_and_embed().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn an_accepted_stop_still_blocks_complete_and_embed_after_the_provider_is_ready()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::quota::__api_run_an_accepted_stop_still_blocks_complete_and_embed_after_the_provider_is_ready().await
            }
        }
        #[cfg(test)]
        mod reference_previews {
            #[cfg(test)]
            #[tokio::test]
            async fn reference_live_stat_fetch_and_delivery_keep_raw_identity_physical_source_and_exactly_once_io()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::reference_previews::__api_run_reference_live_stat_fetch_and_delivery_keep_raw_identity_physical_source_and_exactly_once_io().await
            }
        }
        #[cfg(test)]
        mod selection {
            #[cfg(test)]
            #[tokio::test]
            async fn a_real_guarded_selection_and_gc_can_finish_before_the_old_cancelled_dispatcher_adoption_without_a_late_effect()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::selection::__api_run_a_real_guarded_selection_and_gc_can_finish_before_the_old_cancelled_dispatcher_adoption_without_a_late_effect().await
            }
        }
        #[cfg(test)]
        mod skill_creations {
            #[cfg(test)]
            #[tokio::test]
            async fn native_skill_sources_keep_actual_environment_or_host_raw_origin_and_once_only_supplier_work()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::skill_creations::__api_run_native_skill_sources_keep_actual_environment_or_host_raw_origin_and_once_only_supplier_work().await
            }
        }
        #[cfg(test)]
        mod snapshot_reads {
            #[cfg(test)]
            #[tokio::test]
            async fn ordinary_snapshot_http_read_has_an_actual_native_journal_result()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::snapshot_reads::__api_run_ordinary_snapshot_http_read_has_an_actual_native_journal_result().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn revoked_queued_snapshot_http_read_never_reaches_native_accepted_after_reconnect()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::snapshot_reads::__api_run_revoked_queued_snapshot_http_read_never_reaches_native_accepted_after_reconnect().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn queued_snapshot_read_cannot_rebind_a_new_custodian_generation_to_its_old_wire_command()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::snapshot_reads::__api_run_queued_snapshot_read_cannot_rebind_a_new_custodian_generation_to_its_old_wire_command().await
            }
        }
        #[cfg(test)]
        mod terminal_close {
            #[cfg(test)]
            #[tokio::test]
            async fn new_scoped_terminal_close_refuses_unaccepted_legacy_none_without_cancelling_old_work()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::terminal_close::__api_run_new_scoped_terminal_close_refuses_unaccepted_legacy_none_without_cancelling_old_work().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn new_scoped_attempt_never_relabels_accepted_legacy_none_or_vetoes_its_result()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::terminal_close::__api_run_new_scoped_attempt_never_relabels_accepted_legacy_none_or_vetoes_its_result().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn captured_terminal_close_rechecks_its_real_key_before_first_wire()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::terminal_close::__api_run_captured_terminal_close_rechecks_its_real_key_before_first_wire().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn captured_terminal_close_rechecks_its_actual_environment_generation_before_first_wire()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::terminal_close::__api_run_captured_terminal_close_rechecks_its_actual_environment_generation_before_first_wire().await
            }
        }
        #[cfg(test)]
        mod transport {
            #[cfg(test)]
            #[tokio::test]
            async fn two_forwarded_turns_use_the_production_command_port_and_rebind_without_a_durable_api_body()
            -> ::orch_api_base::backplane::command_admission_tests::Proof {
                orch_api_base::backplane::command_admission_tests::transport::__api_run_two_forwarded_turns_use_the_production_command_port_and_rebind_without_a_durable_api_body().await
            }
        }
        #[cfg(test)]
        #[tokio::test]
        async fn an_already_connected_dispatcher_adopts_a_retained_turn_start_only_when_issued()
        -> ::orch_api_base::backplane::command_admission_tests::Proof {
            orch_api_base::backplane::command_admission_tests::__api_run_an_already_connected_dispatcher_adopts_a_retained_turn_start_only_when_issued().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_cancelled_queued_row_cannot_be_adopted_after_its_actual_native_completion_probe()
        -> ::orch_api_base::backplane::command_admission_tests::Proof {
            orch_api_base::backplane::command_admission_tests::__api_run_a_cancelled_queued_row_cannot_be_adopted_after_its_actual_native_completion_probe().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_stale_startup_open_row_cannot_resurrect_after_cancellation_before_entry_publication()
        -> ::orch_api_base::backplane::command_admission_tests::Proof {
            orch_api_base::backplane::command_admission_tests::__api_run_a_stale_startup_open_row_cannot_resurrect_after_cancellation_before_entry_publication().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_probe_after_cancellation_cannot_pass_the_write_already_owned_by_its_output_iteration()
        -> ::orch_api_base::backplane::command_admission_tests::Proof {
            orch_api_base::backplane::command_admission_tests::__api_run_a_probe_after_cancellation_cannot_pass_the_write_already_owned_by_its_output_iteration().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_retired_output_task_cannot_publish_after_the_current_entry_has_completed_its_probe()
        -> ::orch_api_base::backplane::command_admission_tests::Proof {
            orch_api_base::backplane::command_admission_tests::__api_run_a_retired_output_task_cannot_publish_after_the_current_entry_has_completed_its_probe().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn only_an_explicit_live_observation_owner_may_use_its_existing_non_durable_read_route()
        -> ::orch_api_base::backplane::command_admission_tests::Proof {
            orch_api_base::backplane::command_admission_tests::__api_run_only_an_explicit_live_observation_owner_may_use_its_existing_non_durable_read_route().await
        }
    }
}

pub mod blobs {

    pub use orch_api_base::blobs::UPLOADS_DIR;

    pub use orch_api_base::blobs::BlobsError;

    pub use orch_api_base::blobs::Blobs;

    pub use orch_api_base::blobs::Transfer;

    pub use orch_api_base::blobs::TooManyTransfers;

    pub use orch_api_base::blobs::PieceFile;

    pub use orch_api_base::blobs::prepare_piece;

    pub use orch_api_base::blobs::write_piece;

    pub use orch_api_base::blobs::discard;

    pub use orch_api_base::blobs::MemoryBlobs;
    #[cfg(test)]
    mod tests {
        #[cfg(test)]
        #[tokio::test]
        async fn a_staged_request_is_unpublished_and_its_body_owns_the_temporary_file_until_consumed()
         {
            orch_api_base::blobs::tests::__api_run_a_staged_request_is_unpublished_and_its_body_owns_the_temporary_file_until_consumed().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn preparing_a_piece_keeps_existing_bytes_until_the_native_write_is_admitted() {
            orch_api_base::blobs::tests::__api_run_preparing_a_piece_keeps_existing_bytes_until_the_native_write_is_admitted().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_machine_runs_four_transfers_at_once_and_a_fifth_waits_for_one_to_end() {
            orch_api_base::blobs::tests::__api_run_a_machine_runs_four_transfers_at_once_and_a_fifth_waits_for_one_to_end().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_body_is_stored_under_the_hash_it_has_and_one_that_is_too_large_leaves_nothing() {
            orch_api_base::blobs::tests::__api_run_a_body_is_stored_under_the_hash_it_has_and_one_that_is_too_large_leaves_nothing().await
        }
    }
}

pub mod cache {

    pub use orch_api_base::cache::MANIFEST_MAX_STORED_BYTES;

    pub use orch_api_base::cache::STALE_UPLOAD_AGE;

    pub use orch_api_base::cache::EVICTION_EVERY;

    pub use orch_api_base::cache::COLLECTION_EVERY;

    pub use orch_api_base::cache::EVICTION_JOB;

    pub use orch_api_base::cache::COLLECTION_JOB;

    pub use orch_api_base::cache::DEFAULT_CACHE_QUOTA_BYTES;

    pub use orch_api_base::cache::Watching;

    pub use orch_api_base::cache::Samples;

    pub use orch_api_base::cache::Cache;

    pub use orch_api_base::cache::Evicted;

    pub use orch_api_base::cache::evict_caches;

    pub use orch_api_base::cache::Collected;

    pub use orch_api_base::cache::collect_garbage;

    pub use orch_api_base::cache::start_housekeeping;

    pub use orch_api_base::cache::Entry;

    pub use orch_api_base::cache::EntryKind;

    pub use orch_api_base::cache::Header;

    pub use orch_api_base::cache::Manifest;

    pub use orch_api_base::cache::ManifestError;
    #[cfg(test)]
    mod tests {
        #[cfg(test)]
        #[tokio::test]
        async fn a_job_runs_in_one_process_at_a_time_under_its_lease_and_keeps_no_transaction_open()
        {
            orch_api_base::cache::tests::__api_run_a_job_runs_in_one_process_at_a_time_under_its_lease_and_keeps_no_transaction_open().await
        }
    }
}

pub mod certificates {

    pub use orch_api_base::certificates::CLIENT_IP_HEADER;

    pub use orch_api_base::certificates::CLIENT_CERTIFICATE_HEADER;

    pub use orch_api_base::certificates::FORWARDED_HOST_HEADER;

    pub use orch_api_base::certificates::BOOTSTRAP_TOKEN_SECS;

    pub use orch_api_base::certificates::CertificateSettings;

    pub use orch_api_base::certificates::LinkFault;

    pub use orch_api_base::certificates::TokenFor;

    pub use orch_api_base::certificates::TokenRequest;

    pub use orch_api_base::certificates::NewToken;

    pub use orch_api_base::certificates::Adopted;

    pub use orch_api_base::certificates::RenewCaller;

    pub use orch_api_base::certificates::CertificateService;

    pub use orch_api_base::certificates::internal_routes;

    pub use orch_api_base::certificates::RENEW_CERTIFICATE_HEADER;

    pub use orch_api_base::certificates::RENEW_SENT_AT_HEADER;

    pub use orch_api_base::certificates::RENEW_SIGNATURE_HEADER;
    #[cfg(test)]
    mod frontend {
        #[cfg(all(test, unix))]
        mod tests {
            #[cfg(test)]
            #[test]
            fn the_existing_authority_issues_and_renews_the_front_with_its_names_as_a_complete_pair()
             {
                orch_api_base::certificates::frontend::tests::__api_run_the_existing_authority_issues_and_renews_the_front_with_its_names_as_a_complete_pair()
            }
        }
    }
    #[cfg(test)]
    mod revocation {
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[tokio::test]
            async fn rejected_revocation_cut_leaves_registration_commands_and_audit_unchanged() {
                orch_api_base::certificates::revocation::tests::__api_run_rejected_revocation_cut_leaves_registration_commands_and_audit_unchanged().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn revocation_packet_keeps_its_transaction_and_native_once_only_cancellation() {
                orch_api_base::certificates::revocation::tests::__api_run_revocation_packet_keeps_its_transaction_and_native_once_only_cancellation().await
            }
        }
    }
    #[cfg(test)]
    mod prepared_host_tokens {
        #[cfg(test)]
        #[tokio::test]
        async fn a_host_token_cut_precedes_its_ledger_and_keeps_exact_choices_on_success() {
            orch_api_base::certificates::prepared_host_tokens::__api_run_a_host_token_cut_precedes_its_ledger_and_keeps_exact_choices_on_success().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_host_token_reservation_cannot_cross_its_transaction_or_authority_before_the_cut()
        {
            orch_api_base::certificates::prepared_host_tokens::__api_run_a_host_token_reservation_cannot_cross_its_transaction_or_authority_before_the_cut().await
        }
    }
}

pub mod content {

    pub use orch_api_base::content::MACHINE_HEADER;

    pub use orch_api_base::content::DEFAULT_MAX_BLOB_BYTES;

    pub use orch_api_base::content::MAX_BLOB_KEY;

    pub use orch_api_base::content::routes;
    #[cfg(test)]
    #[test]
    fn native_result_uploads_require_the_exact_capture_command_and_typed_opt_in() {
        orch_api_base::content::__api_run_native_result_uploads_require_the_exact_capture_command_and_typed_opt_in()
    }
}

pub mod dispatch {

    pub use orch_api_base::dispatch::KEY_MEMBER;

    pub use orch_api_base::dispatch::DispatchError;

    pub use orch_api_base::dispatch::Issued;

    pub use orch_api_base::dispatch::CommandOutcome;

    pub use orch_api_base::dispatch::RunError;

    pub use orch_api_base::dispatch::StoredCommand;

    pub use orch_api_base::dispatch::cmd_of;

    pub use orch_api_base::dispatch::Dispatcher;

    pub use orch_api_base::dispatch::SecretFault;

    pub use orch_api_base::dispatch::SecretSource;

    pub use orch_api_base::dispatch::StoredSecrets;
    #[cfg(test)]
    mod tests {
        #[cfg(test)]
        #[test]
        fn a_stored_command_carries_its_key_and_gives_it_back() {
            orch_api_base::dispatch::tests::__api_run_a_stored_command_carries_its_key_and_gives_it_back()
        }
        #[cfg(test)]
        #[test]
        fn a_command_without_a_key_has_none() {
            orch_api_base::dispatch::tests::__api_run_a_command_without_a_key_has_none()
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_ticket_request_in_a_command_is_signed_when_the_command_is_sent() {
            orch_api_base::dispatch::tests::__api_run_a_ticket_request_in_a_command_is_signed_when_the_command_is_sent().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_secret_that_the_settings_stored_is_sealed_to_the_machine_for_the_command_that_names_it()
         {
            orch_api_base::dispatch::tests::__api_run_a_secret_that_the_settings_stored_is_sealed_to_the_machine_for_the_command_that_names_it().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_command_that_cannot_be_made_whole_is_not_sent_and_says_why() {
            orch_api_base::dispatch::tests::__api_run_a_command_that_cannot_be_made_whole_is_not_sent_and_says_why().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn the_answer_of_a_read_is_cleared_once_had_and_a_late_reader_is_told_that_it_expired()
         {
            orch_api_base::dispatch::tests::__api_run_the_answer_of_a_read_is_cleared_once_had_and_a_late_reader_is_told_that_it_expired().await
        }
    }
}

pub mod documents_upkeep {

    pub use orch_api_base::documents_upkeep::VERSIONS_JOB;

    pub use orch_api_base::documents_upkeep::PURGE_JOB;

    pub use orch_api_base::documents_upkeep::VERSIONS_EVERY;

    pub use orch_api_base::documents_upkeep::PURGE_EVERY;

    pub use orch_api_base::documents_upkeep::Purged;

    pub use orch_api_base::documents_upkeep::cut_versions;

    pub use orch_api_base::documents_upkeep::purge;

    pub use orch_api_base::documents_upkeep::start;
    #[cfg(test)]
    mod tests {
        #[cfg(test)]
        #[test]
        fn a_span_longer_than_time_can_say_reaches_back_to_the_beginning_and_never_fails() {
            orch_api_base::documents_upkeep::tests::__api_run_a_span_longer_than_time_can_say_reaches_back_to_the_beginning_and_never_fails()
        }
    }
}

pub mod harness_channels {

    pub use orch_api_base::harness_channels::mirrored;

    pub use orch_api_base::harness_channels::readable;

    pub use orch_api_base::harness_channels::read;

    pub use orch_api_base::harness_channels::read_exact;

    pub use orch_api_base::harness_channels::read_exact_configured;

    pub use orch_api_base::harness_channels::trusts_mirror;

    pub use orch_api_base::harness_channels::checksum_trusted;

    pub use orch_api_base::harness_channels::read_for;

    pub use orch_api_base::harness_channels::read_configured;

    pub use orch_api_base::harness_channels::read_for_configured;

    pub use orch_api_base::harness_channels::download_url;

    pub use orch_api_base::harness_channels::channel_key;

    pub use orch_api_base::harness_channels::keep;

    pub use orch_api_base::harness_channels::keep_before;

    pub use orch_api_base::harness_channels::kept_keys;

    pub use orch_api_base::harness_channels::status;

    pub use orch_api_base::harness_channels::record_read;

    pub use orch_api_base::harness_channels::scheduled_look;

    pub use orch_api_base::harness_channels::keep_reading;
    #[cfg(test)]
    mod github {
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn releases_give_their_published_linux_packages_with_the_checksums_github_keeps() {
                orch_api_base::harness_channels::github::tests::__api_run_releases_give_their_published_linux_packages_with_the_checksums_github_keeps()
            }
            #[cfg(test)]
            #[test]
            fn a_checksum_file_of_the_release_stands_for_the_digest_where_the_manifest_says_so() {
                orch_api_base::harness_channels::github::tests::__api_run_a_checksum_file_of_the_release_stands_for_the_digest_where_the_manifest_says_so()
            }
        }
    }
    #[cfg(test)]
    mod npm {
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn a_version_counts_only_when_one_of_the_registrys_keys_verifies_its_signature() {
                orch_api_base::harness_channels::npm::tests::__api_run_a_version_counts_only_when_one_of_the_registrys_keys_verifies_its_signature()
            }
            #[cfg(test)]
            #[test]
            fn the_tags_version_comes_first_then_the_newest_without_pre_releases() {
                orch_api_base::harness_channels::npm::tests::__api_run_the_tags_version_comes_first_then_the_newest_without_pre_releases()
            }
            #[cfg(test)]
            #[test]
            fn a_scoped_package_is_asked_for_with_its_slash_written_out() {
                orch_api_base::harness_channels::npm::tests::__api_run_a_scoped_package_is_asked_for_with_its_slash_written_out()
            }
        }
    }
    #[cfg(test)]
    mod pypi {
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn a_release_gives_a_wheel_for_each_processor_with_the_checksum_the_index_keeps() {
                orch_api_base::harness_channels::pypi::tests::__api_run_a_release_gives_a_wheel_for_each_processor_with_the_checksum_the_index_keeps()
            }
            #[cfg(test)]
            #[test]
            fn a_release_the_index_names_in_pep_440_is_kept_under_the_name_a_profile_pins() {
                orch_api_base::harness_channels::pypi::tests::__api_run_a_release_the_index_names_in_pep_440_is_kept_under_the_name_a_profile_pins()
            }
            #[cfg(test)]
            #[test]
            fn an_exact_release_reads_the_files_the_index_lists_for_it() {
                orch_api_base::harness_channels::pypi::tests::__api_run_an_exact_release_reads_the_files_the_index_lists_for_it()
            }
        }
    }
    #[cfg(test)]
    mod signed {
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn a_manifest_gives_its_linux_programs_with_their_checksums() {
                orch_api_base::harness_channels::signed::tests::__api_run_a_manifest_gives_its_linux_programs_with_their_checksums()
            }
            #[cfg(test)]
            #[test]
            fn a_vendors_signature_counts_only_when_it_verifies() {
                orch_api_base::harness_channels::signed::tests::__api_run_a_vendors_signature_counts_only_when_it_verifies()
            }
        }
    }
    #[cfg(test)]
    mod tests {
        #[cfg(test)]
        #[test]
        fn old_channel_rows_have_no_checksum_authority_and_mirror_trust_is_explicit() {
            orch_api_base::harness_channels::tests::__api_run_old_channel_rows_have_no_checksum_authority_and_mirror_trust_is_explicit()
        }
        #[cfg(test)]
        #[test]
        fn codex_releases_give_their_published_linux_packages_with_the_checksums_github_keeps() {
            orch_api_base::harness_channels::tests::__api_run_codex_releases_give_their_published_linux_packages_with_the_checksums_github_keeps()
        }
        #[cfg(test)]
        #[test]
        fn a_claude_code_manifest_gives_its_linux_programs_with_their_checksums() {
            orch_api_base::harness_channels::tests::__api_run_a_claude_code_manifest_gives_its_linux_programs_with_their_checksums()
        }
        #[cfg(test)]
        #[test]
        fn a_mirror_serves_each_vendor_under_its_host() {
            orch_api_base::harness_channels::tests::__api_run_a_mirror_serves_each_vendor_under_its_host()
        }
        #[cfg(test)]
        #[test]
        fn a_pattern_may_stand_for_what_varies_in_a_file_name() {
            orch_api_base::harness_channels::tests::__api_run_a_pattern_may_stand_for_what_varies_in_a_file_name()
        }
        #[cfg(test)]
        #[test]
        fn a_build_that_is_kept_is_known_by_its_key_for_its_version_and_processor_alone() {
            orch_api_base::harness_channels::tests::__api_run_a_build_that_is_kept_is_known_by_its_key_for_its_version_and_processor_alone()
        }
        #[cfg(test)]
        #[test]
        fn the_programs_orch_ships_name_a_channel_each_but_the_fake() {
            orch_api_base::harness_channels::tests::__api_run_the_programs_orch_ships_name_a_channel_each_but_the_fake()
        }
    }
}

pub mod host_ports {

    pub use orch_api_base::host_ports::Proposals;

    pub use orch_api_base::host_ports::proposal_of;

    pub use orch_api_base::host_ports::ask;

    pub use orch_api_base::host_ports::REPUBLISH_EVERY;

    pub use orch_api_base::host_ports::REPUBLISH_AFTER_START;

    pub use orch_api_base::host_ports::PublishFault;

    pub use orch_api_base::host_ports::publish;

    pub use orch_api_base::host_ports::publish_in_background;

    pub use orch_api_base::host_ports::unpublish;

    pub use orch_api_base::host_ports::unpublish_in_background;

    pub use orch_api_base::host_ports::keep_published;

    pub use orch_api_base::host_ports::follow_environments;

    pub use orch_api_base::host_ports::ask_in_background;
    #[cfg(test)]
    mod tests {
        #[cfg(test)]
        #[test]
        fn applications_never_share_proposals_and_a_cancelled_read_can_be_tried_again() {
            orch_api_base::host_ports::tests::__api_run_applications_never_share_proposals_and_a_cancelled_read_can_be_tried_again()
        }
    }
}

pub mod hosting {

    pub use orch_api_base::hosting::Build;

    pub use orch_api_base::hosting::BuildCatalog;

    pub use orch_api_base::hosting::ListedBuilds;

    pub use orch_api_base::hosting::ReleaseBuilds;

    pub use orch_api_base::hosting::ApiHostServices;
    #[cfg(test)]
    mod upgrade_tests {
        #[cfg(test)]
        #[test]
        fn every_program_resolves_shipped_checked_newest_and_exact_without_retargeting_a_recorded_pin()
         {
            orch_api_base::hosting::upgrade_tests::__api_run_every_program_resolves_shipped_checked_newest_and_exact_without_retargeting_a_recorded_pin()
        }
        #[cfg(test)]
        #[test]
        fn checksum_refusal_remains_a_refusal_for_every_program_and_policy() {
            orch_api_base::hosting::upgrade_tests::__api_run_checksum_refusal_remains_a_refusal_for_every_program_and_policy()
        }
        #[cfg(test)]
        #[test]
        fn mixed_machine_versions_are_independent_while_one_unit_cannot_satisfy_conflicting_pins() {
            orch_api_base::hosting::upgrade_tests::__api_run_mixed_machine_versions_are_independent_while_one_unit_cannot_satisfy_conflicting_pins()
        }
        #[cfg(test)]
        #[test]
        fn a_python_index_pre_release_pin_keeps_the_canonical_existing_build_identity() {
            orch_api_base::hosting::upgrade_tests::__api_run_a_python_index_pre_release_pin_keeps_the_canonical_existing_build_identity()
        }
    }
    #[cfg(test)]
    mod tests {
        #[cfg(test)]
        #[test]
        fn a_profile_pin_selects_the_listed_nondefault_build_and_never_falls_back() {
            orch_api_base::hosting::tests::__api_run_a_profile_pin_selects_the_listed_nondefault_build_and_never_falls_back()
        }
        #[cfg(test)]
        #[test]
        fn a_place_asking_for_the_newest_gets_the_newest_checked_or_any_and_never_a_pre_release_order_by_text()
         {
            orch_api_base::hosting::tests::__api_run_a_place_asking_for_the_newest_gets_the_newest_checked_or_any_and_never_a_pre_release_order_by_text()
        }
        #[cfg(test)]
        #[test]
        fn a_release_is_after_its_pre_releases_and_pre_releases_go_by_kind_and_number() {
            orch_api_base::hosting::tests::__api_run_a_release_is_after_its_pre_releases_and_pre_releases_go_by_kind_and_number()
        }
        #[cfg(test)]
        #[test]
        fn the_builds_for_a_platform_are_those_listed_for_it_with_the_connector_first() {
            orch_api_base::hosting::tests::__api_run_the_builds_for_a_platform_are_those_listed_for_it_with_the_connector_first()
        }
        #[cfg(test)]
        #[test]
        fn the_builds_of_a_unit_are_the_connector_then_the_harnesses_each_name_once() {
            orch_api_base::hosting::tests::__api_run_the_builds_of_a_unit_are_the_connector_then_the_harnesses_each_name_once()
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_unit_runs_the_connector_of_the_release_at_the_address_anybody_downloads_it_from()
        {
            orch_api_base::hosting::tests::__api_run_a_unit_runs_the_connector_of_the_release_at_the_address_anybody_downloads_it_from().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_connector_that_was_replaced_is_another_version_with_a_hash_of_its_own() {
            orch_api_base::hosting::tests::__api_run_a_connector_that_was_replaced_is_another_version_with_a_hash_of_its_own().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_system_that_was_not_started_from_a_release_has_no_connector_to_put_in_a_unit() {
            orch_api_base::hosting::tests::__api_run_a_system_that_was_not_started_from_a_release_has_no_connector_to_put_in_a_unit().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_release_without_a_connector_for_the_platform_of_the_host_has_nothing_to_put_in_a_unit()
         {
            orch_api_base::hosting::tests::__api_run_a_release_without_a_connector_for_the_platform_of_the_host_has_nothing_to_put_in_a_unit().await
        }
        #[cfg(test)]
        #[test]
        fn a_platform_without_a_connector_build_has_nothing_to_put_in_a_unit() {
            orch_api_base::hosting::tests::__api_run_a_platform_without_a_connector_build_has_nothing_to_put_in_a_unit()
        }
    }
}

pub mod input_schedules {

    pub use orch_api_base::input_schedules::mount;

    pub use orch_api_base::input_schedules::install;

    pub use orch_api_base::input_schedules::run_due;

    pub use orch_api_base::input_schedules::start;
    #[cfg(test)]
    mod cron {
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn explicit_cron_steps_bounds_and_calendar_rules_have_stable_occurrences() {
                orch_api_base::input_schedules::cron::tests::__api_run_explicit_cron_steps_bounds_and_calendar_rules_have_stable_occurrences()
            }
        }
    }
    #[cfg(test)]
    mod timing {
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn a_gap_is_a_visible_skip_and_a_fold_has_only_the_earlier_occurrence() {
                orch_api_base::input_schedules::timing::tests::__api_run_a_gap_is_a_visible_skip_and_a_fold_has_only_the_earlier_occurrence()
            }
            #[cfg(test)]
            #[test]
            fn a_backlog_coalesces_and_resume_chooses_only_future_work() {
                orch_api_base::input_schedules::timing::tests::__api_run_a_backlog_coalesces_and_resume_chooses_only_future_work()
            }
        }
    }
}
#[cfg(test)]
pub(crate) mod key_operations {
    #[cfg(test)]
    pub(crate) mod activity_store {
        #[cfg(test)]
        #[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
        mod tests {
            #[cfg(test)]
            #[tokio::test]
            async fn the_shared_context_keeps_explicit_replay_and_outcome_once_without_raw_inputs()
            {
                orch_api_base::key_operations::activity_store::tests::__api_run_the_shared_context_keeps_explicit_replay_and_outcome_once_without_raw_inputs().await
            }
        }
    }
    #[cfg(test)]
    pub(crate) mod admin {
        #[cfg(test)]
        #[allow(clippy::unwrap_used, reason = "test assertions fail by panicking")]
        mod tests {
            #[cfg(test)]
            #[test]
            fn browser_mode_and_current_standing_are_preserved() {
                orch_api_base::key_operations::admin::tests::__api_run_browser_mode_and_current_standing_are_preserved()
            }
            #[cfg(test)]
            #[test]
            fn exact_system_read_is_admitted_without_private_override() {
                orch_api_base::key_operations::admin::tests::__api_run_exact_system_read_is_admitted_without_private_override()
            }
            #[cfg(test)]
            #[test]
            fn targeted_administration_does_not_require_an_artificial_system_target() {
                orch_api_base::key_operations::admin::tests::__api_run_targeted_administration_does_not_require_an_artificial_system_target()
            }
            #[cfg(test)]
            #[test]
            fn legacy_or_unadmitted_credentials_do_not_gain_admin_operations() {
                orch_api_base::key_operations::admin::tests::__api_run_legacy_or_unadmitted_credentials_do_not_gain_admin_operations()
            }
            #[cfg(test)]
            #[test]
            fn administration_accepts_real_empty_roles_and_keeps_optional_read_on_current_readmission()
             {
                orch_api_base::key_operations::admin::tests::__api_run_administration_accepts_real_empty_roles_and_keeps_optional_read_on_current_readmission()
            }
            #[cfg(test)]
            #[test]
            fn mixed_empty_administration_requires_each_canonical_role_not_a_matching_flat_target()
            {
                orch_api_base::key_operations::admin::tests::__api_run_mixed_empty_administration_requires_each_canonical_role_not_a_matching_flat_target()
            }
            #[cfg(test)]
            #[test]
            fn node_settings_proof_never_covers_another_node_or_an_environment_provider_or_private_need()
             {
                orch_api_base::key_operations::admin::tests::__api_run_node_settings_proof_never_covers_another_node_or_an_environment_provider_or_private_need()
            }
        }
    }
    #[cfg(test)]
    pub(crate) mod admission {
        #[cfg(test)]
        mod origin_tests {
            #[cfg(test)]
            #[test]
            fn a_wider_incoming_key_cannot_replace_the_restored_native_source_boundary() {
                orch_api_base::key_operations::admission::origin_tests::__api_run_a_wider_incoming_key_cannot_replace_the_restored_native_source_boundary()
            }
            #[cfg(test)]
            #[test]
            fn narrowed_native_admission_retains_both_keys_and_never_adds_private_mode() {
                orch_api_base::key_operations::admission::origin_tests::__api_run_narrowed_native_admission_retains_both_keys_and_never_adds_private_mode()
            }
            #[cfg(test)]
            #[test]
            fn a_restored_different_account_is_not_a_valid_native_operation_boundary() {
                orch_api_base::key_operations::admission::origin_tests::__api_run_a_restored_different_account_is_not_a_valid_native_operation_boundary()
            }
            #[cfg(test)]
            #[test]
            fn an_adoption_identity_prefix_retains_the_initial_narrower_credential_bounds() {
                orch_api_base::key_operations::admission::origin_tests::__api_run_an_adoption_identity_prefix_retains_the_initial_narrower_credential_bounds()
            }
        }
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn a_topic_prefix_never_satisfies_the_normal_complete_operation_gate() {
                orch_api_base::key_operations::admission::tests::__api_run_a_topic_prefix_never_satisfies_the_normal_complete_operation_gate()
            }
            #[cfg(test)]
            #[test]
            fn a_thread_identity_prefix_cannot_skip_its_dependent_creation_effects() {
                orch_api_base::key_operations::admission::tests::__api_run_a_thread_identity_prefix_cannot_skip_its_dependent_creation_effects()
            }
            #[cfg(test)]
            #[test]
            fn required_effects_admit_before_a_future_response_object_exists() {
                orch_api_base::key_operations::admission::tests::__api_run_required_effects_admit_before_a_future_response_object_exists()
            }
            #[cfg(test)]
            #[test]
            fn only_an_explicit_collection_can_be_checked_empty_and_missing_still_refuses() {
                orch_api_base::key_operations::admission::tests::__api_run_only_an_explicit_collection_can_be_checked_empty_and_missing_still_refuses()
            }
            #[cfg(test)]
            #[test]
            fn a_fully_empty_footprint_retains_exact_required_selection_evidence() {
                orch_api_base::key_operations::admission::tests::__api_run_a_fully_empty_footprint_retains_exact_required_selection_evidence()
            }
        }
    }
    #[cfg(test)]
    pub(crate) mod direct {
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn untyped_paths_dynamic_sources_and_compound_variants_remain_closed() {
                orch_api_base::key_operations::direct::tests::__api_run_untyped_paths_dynamic_sources_and_compound_variants_remain_closed()
            }
            #[cfg(test)]
            #[test]
            fn empty_wire_reply_is_a_receipt_with_optional_error_disclosure() {
                orch_api_base::key_operations::direct::tests::__api_run_empty_wire_reply_is_a_receipt_with_optional_error_disclosure()
            }
            #[cfg(test)]
            #[test]
            fn explicit_parent_kind_is_independent_of_business_family_and_parameter_must_exist() {
                orch_api_base::key_operations::direct::tests::__api_run_explicit_parent_kind_is_independent_of_business_family_and_parameter_must_exist()
            }
        }
    }
    #[cfg(test)]
    pub(crate) mod native {
        #[cfg(test)]
        pub(crate) mod effects {
            #[cfg(test)]
            mod common_initial_tests {
                #[cfg(test)]
                mod transactions {
                    #[cfg(test)]
                    #[tokio::test]
                    async fn preliminary_effect_preparation_runs_its_real_http_checkpoint_on_the_same_connection_without_committing_trials() -> ::orch_api_base::key_operations::native::effects::common_initial_tests::TestResult{
                        orch_api_base::key_operations::native::effects::common_initial_tests::transactions::__api_run_preliminary_effect_preparation_runs_its_real_http_checkpoint_on_the_same_connection_without_committing_trials().await
                    }
                    #[cfg(test)]
                    #[tokio::test]
                    async fn effect_preparation_keeps_the_callers_existing_transaction_and_rolls_back_only_with_its_owner() -> ::orch_api_base::key_operations::native::effects::common_initial_tests::TestResult{
                        orch_api_base::key_operations::native::effects::common_initial_tests::transactions::__api_run_effect_preparation_keeps_the_callers_existing_transaction_and_rolls_back_only_with_its_owner().await
                    }
                    #[cfg(test)]
                    #[tokio::test]
                    async fn a_refused_preliminary_checkpoint_leaves_no_trial_and_does_not_erase_current_scope_or_raw_lifetime() -> ::orch_api_base::key_operations::native::effects::common_initial_tests::TestResult{
                        orch_api_base::key_operations::native::effects::common_initial_tests::transactions::__api_run_a_refused_preliminary_checkpoint_leaves_no_trial_and_does_not_erase_current_scope_or_raw_lifetime().await
                    }
                }
                #[cfg(test)]
                #[tokio::test]
                async fn common_initial_retries_a_real_metadata_snapshot_conflict_without_reopening_native_io() -> ::orch_api_base::key_operations::native::effects::common_initial_tests::TestResult{
                    orch_api_base::key_operations::native::effects::common_initial_tests::__api_run_common_initial_retries_a_real_metadata_snapshot_conflict_without_reopening_native_io().await
                }
                #[cfg(test)]
                #[tokio::test]
                async fn common_initial_opens_real_upload_once_for_concurrent_callers_and_pure_repreparation() -> ::orch_api_base::key_operations::native::effects::common_initial_tests::TestResult{
                    orch_api_base::key_operations::native::effects::common_initial_tests::__api_run_common_initial_opens_real_upload_once_for_concurrent_callers_and_pure_repreparation().await
                }
                #[cfg(test)]
                #[tokio::test]
                async fn common_initial_rechecks_raw_identity_and_keeps_original_access_deadline_after_native_wait() -> ::orch_api_base::key_operations::native::effects::common_initial_tests::TestResult{
                    orch_api_base::key_operations::native::effects::common_initial_tests::__api_run_common_initial_rechecks_raw_identity_and_keeps_original_access_deadline_after_native_wait().await
                }
                #[cfg(test)]
                #[tokio::test]
                async fn common_initial_keeps_observed_key_narrowing_even_when_policy_broadens_before_resolution() -> ::orch_api_base::key_operations::native::effects::common_initial_tests::TestResult{
                    orch_api_base::key_operations::native::effects::common_initial_tests::__api_run_common_initial_keeps_observed_key_narrowing_even_when_policy_broadens_before_resolution().await
                }
                #[cfg(test)]
                #[tokio::test]
                async fn common_initial_does_not_retry_a_failed_native_source_open() -> ::orch_api_base::key_operations::native::effects::common_initial_tests::TestResult{
                    orch_api_base::key_operations::native::effects::common_initial_tests::__api_run_common_initial_does_not_retry_a_failed_native_source_open().await
                }
                #[cfg(test)]
                #[tokio::test]
                async fn common_initial_keeps_the_ordinary_upload_custody_gate_before_native_io() -> ::orch_api_base::key_operations::native::effects::common_initial_tests::TestResult{
                    orch_api_base::key_operations::native::effects::common_initial_tests::__api_run_common_initial_keeps_the_ordinary_upload_custody_gate_before_native_io().await
                }
                #[cfg(test)]
                #[tokio::test]
                async fn common_preparation_rechecks_native_custody_without_a_captured_operation() -> ::orch_api_base::key_operations::native::effects::common_initial_tests::TestResult{
                    orch_api_base::key_operations::native::effects::common_initial_tests::__api_run_common_preparation_rechecks_native_custody_without_a_captured_operation().await
                }
                #[cfg(test)]
                #[tokio::test]
                async fn common_preparation_keeps_observed_policy_and_raw_lifetime_minima() -> ::orch_api_base::key_operations::native::effects::common_initial_tests::TestResult{
                    orch_api_base::key_operations::native::effects::common_initial_tests::__api_run_common_preparation_keeps_observed_policy_and_raw_lifetime_minima().await
                }
            }
            #[cfg(test)]
            mod entry_tests {
                #[cfg(test)]
                #[tokio::test]
                async fn declared_adoption_entry_checks_real_prerequisites_without_admitting_future_creator_roles()
                -> ::orch_api_base::key_operations::native::effects::entry_tests::TestResult
                {
                    orch_api_base::key_operations::native::effects::entry_tests::__api_run_declared_adoption_entry_checks_real_prerequisites_without_admitting_future_creator_roles().await
                }
                #[cfg(test)]
                #[tokio::test]
                async fn declared_entry_rejects_each_missing_duplicate_mistyped_or_undeclared_source_before_require()
                -> ::orch_api_base::key_operations::native::effects::entry_tests::TestResult
                {
                    orch_api_base::key_operations::native::effects::entry_tests::__api_run_declared_entry_rejects_each_missing_duplicate_mistyped_or_undeclared_source_before_require().await
                }
                #[cfg(test)]
                #[tokio::test]
                async fn declared_entry_keeps_the_initial_origin_current_scope_and_actual_raw_lifetime()
                -> ::orch_api_base::key_operations::native::effects::entry_tests::TestResult
                {
                    orch_api_base::key_operations::native::effects::entry_tests::__api_run_declared_entry_keeps_the_initial_origin_current_scope_and_actual_raw_lifetime().await
                }
                #[cfg(test)]
                #[tokio::test]
                async fn declared_entry_never_uses_scope_in_place_of_ordinary_authority_or_a_second_step_hold()
                -> ::orch_api_base::key_operations::native::effects::entry_tests::TestResult
                {
                    orch_api_base::key_operations::native::effects::entry_tests::__api_run_declared_entry_never_uses_scope_in_place_of_ordinary_authority_or_a_second_step_hold().await
                }
            }
        }
        #[cfg(test)]
        pub(crate) mod reply {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn a_completed_account_replay_requires_its_declared_response_source_and_no_spent_creation_source()
                -> ::orch_api_base::middleware::requests::ApiResult<()> {
                    orch_api_base::key_operations::native::reply::tests::__api_run_a_completed_account_replay_requires_its_declared_response_source_and_no_spent_creation_source()
                }
                #[cfg(test)]
                #[test]
                fn a_first_account_creation_without_its_declared_required_source_is_refused() {
                    orch_api_base::key_operations::native::reply::tests::__api_run_a_first_account_creation_without_its_declared_required_source_is_refused()
                }
                #[cfg(test)]
                #[test]
                fn additional_native_response_roles_are_explicit_and_cannot_include_management()
                -> ::orch_api_base::middleware::requests::ApiResult<()> {
                    orch_api_base::key_operations::native::reply::tests::__api_run_additional_native_response_roles_are_explicit_and_cannot_include_management()
                }
                #[cfg(test)]
                #[test]
                fn retained_collection_subjects_are_order_independent_but_not_replaceable() {
                    orch_api_base::key_operations::native::reply::tests::__api_run_retained_collection_subjects_are_order_independent_but_not_replaceable()
                }
            }
        }
        #[cfg(test)]
        pub(crate) mod resource_reply {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn collection_read_needs_every_actual_subject_and_never_readmits_completed_manage()
                {
                    orch_api_base::key_operations::native::resource_reply::tests::__api_run_collection_read_needs_every_actual_subject_and_never_readmits_completed_manage()
                }
                #[cfg(test)]
                #[test]
                fn empty_content_is_not_a_missing_role_a_singleton_or_a_new_permission() {
                    orch_api_base::key_operations::native::resource_reply::tests::__api_run_empty_content_is_not_a_missing_role_a_singleton_or_a_new_permission()
                }
            }
        }
        #[cfg(test)]
        #[allow(
            clippy::expect_used,
            reason = "fixture invariants and test assertions fail by panicking"
        )]
        mod tests {
            #[cfg(test)]
            #[test]
            fn stored_declaration_keeps_manage_and_response_read_independent() {
                orch_api_base::key_operations::native::tests::__api_run_stored_declaration_keeps_manage_and_response_read_independent()
            }
            #[cfg(test)]
            #[test]
            fn missing_sources_and_wrong_cardinality_never_become_empty_proof() {
                orch_api_base::key_operations::native::tests::__api_run_missing_sources_and_wrong_cardinality_never_become_empty_proof()
            }
            #[cfg(test)]
            #[test]
            fn one_native_role_binds_all_of_its_canonical_required_actions() {
                orch_api_base::key_operations::native::tests::__api_run_one_native_role_binds_all_of_its_canonical_required_actions()
            }
            #[cfg(test)]
            #[test]
            fn undeclared_business_kind_and_duplicate_source_are_rejected() {
                orch_api_base::key_operations::native::tests::__api_run_undeclared_business_kind_and_duplicate_source_are_rejected()
            }
            #[cfg(test)]
            #[test]
            fn known_preparation_does_not_replace_an_unresolved_required_collection() {
                orch_api_base::key_operations::native::tests::__api_run_known_preparation_does_not_replace_an_unresolved_required_collection()
            }
            #[cfg(test)]
            #[test]
            fn deferred_response_binding_keeps_required_subjects_complete_and_optional_sources_unproven()
             {
                orch_api_base::key_operations::native::tests::__api_run_deferred_response_binding_keeps_required_subjects_complete_and_optional_sources_unproven()
            }
        }
    }
    #[cfg(test)]
    pub(crate) mod publication {
        #[cfg(test)]
        #[allow(clippy::unwrap_used, reason = "a control fails by panicking")]
        mod tests {
            #[cfg(test)]
            #[test]
            fn an_unaccepted_refusal_never_replaces_an_accepted_receipt_or_a_different_error() {
                orch_api_base::key_operations::publication::tests::__api_run_an_unaccepted_refusal_never_replaces_an_accepted_receipt_or_a_different_error()
            }
            #[cfg(test)]
            #[test]
            fn sparse_failure_keeps_native_status_code_retry_and_trusted_cause() {
                orch_api_base::key_operations::publication::tests::__api_run_sparse_failure_keeps_native_status_code_retry_and_trusted_cause()
            }
        }
    }
    #[cfg(test)]
    pub(crate) mod resolver {
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn raw_query_keeps_the_real_byte_body_and_native_offset_decoder() {
                orch_api_base::key_operations::resolver::tests::__api_run_raw_query_keeps_the_real_byte_body_and_native_offset_decoder()
            }
            #[cfg(test)]
            #[test]
            fn raw_query_rejects_an_invented_body_read_mode_or_unbounded_retained_query() {
                orch_api_base::key_operations::resolver::tests::__api_run_raw_query_rejects_an_invented_body_read_mode_or_unbounded_retained_query()
            }
            #[cfg(test)]
            #[test]
            fn retained_opened_source_survives_fresh_preparation_and_never_retargets() {
                orch_api_base::key_operations::resolver::tests::__api_run_retained_opened_source_survives_fresh_preparation_and_never_retargets()
            }
            #[cfg(test)]
            #[test]
            fn poisoned_opened_source_cannot_downgrade_to_unopened() {
                orch_api_base::key_operations::resolver::tests::__api_run_poisoned_opened_source_cannot_downgrade_to_unopened()
            }
            #[cfg(test)]
            #[test]
            fn unclassified_ordinary_path_never_admits_a_modern_key_or_retains_operation_proof() {
                orch_api_base::key_operations::resolver::tests::__api_run_unclassified_ordinary_path_never_admits_a_modern_key_or_retains_operation_proof()
            }
            #[cfg(test)]
            #[test]
            fn pending_completion_is_explicit_and_denied_never_widens() {
                orch_api_base::key_operations::resolver::tests::__api_run_pending_completion_is_explicit_and_denied_never_widens()
            }
            #[cfg(test)]
            #[test]
            fn prepared_operands_are_redacted_and_guard_snapshots_do_not_follow_later_replacement()
            {
                orch_api_base::key_operations::resolver::tests::__api_run_prepared_operands_are_redacted_and_guard_snapshots_do_not_follow_later_replacement()
            }
        }
    }
    #[cfg(test)]
    pub(crate) mod targets {
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn typed_current_ancestry_comes_from_the_graph_and_same_id_does_not_change_kind() {
                orch_api_base::key_operations::targets::tests::__api_run_typed_current_ancestry_comes_from_the_graph_and_same_id_does_not_change_kind()
            }
            #[cfg(test)]
            #[test]
            fn resource_home_is_placement_even_when_its_site_has_no_node_path() {
                orch_api_base::key_operations::targets::tests::__api_run_resource_home_is_placement_even_when_its_site_has_no_node_path()
            }
            #[cfg(test)]
            #[test]
            fn document_uses_its_actual_beside_node_without_inventing_folder_ancestry() {
                orch_api_base::key_operations::targets::tests::__api_run_document_uses_its_actual_beside_node_without_inventing_folder_ancestry()
            }
            #[cfg(test)]
            #[test]
            fn creation_family_does_not_relabel_its_real_parent() {
                orch_api_base::key_operations::targets::tests::__api_run_creation_family_does_not_relabel_its_real_parent()
            }
            #[cfg(test)]
            #[test]
            fn bindings_cannot_silently_replace_resolved_targets() {
                orch_api_base::key_operations::targets::tests::__api_run_bindings_cannot_silently_replace_resolved_targets()
            }
        }
    }
    #[cfg(test)]
    pub(crate) mod completed_projection_tests {
        #[cfg(test)]
        #[tokio::test]
        async fn completed_projection_provider_update_keeps_accepted_revision_after_real_snapshot_retry()
         {
            orch_api_base::key_operations::completed_projection_tests::__api_run_completed_projection_provider_update_keeps_accepted_revision_after_real_snapshot_retry().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn completed_projection_model_create_keeps_accepted_model_after_real_snapshot_retry()
        {
            orch_api_base::key_operations::completed_projection_tests::__api_run_completed_projection_model_create_keeps_accepted_model_after_real_snapshot_retry().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn completed_projection_actual_http_session_expiry_withholds_full_but_acknowledges_write()
         {
            orch_api_base::key_operations::completed_projection_tests::__api_run_completed_projection_actual_http_session_expiry_withholds_full_but_acknowledges_write().await
        }
    }
}

pub mod local_ports {
    #[cfg(test)]
    mod pending_tests {
        #[cfg(test)]
        #[test]
        fn pending_read_admission_is_per_environment_and_cancellation_releases_its_room() {
            orch_api_base::local_ports::pending_tests::__api_run_pending_read_admission_is_per_environment_and_cancellation_releases_its_room()
        }
    }
}

pub mod media {

    pub use orch_api_base::media::NAME_MAX_CHARS;

    pub use orch_api_base::media::shows_inline;

    pub use orch_api_base::media::safe_name;

    pub use orch_api_base::media::DIRECTORY;

    pub use orch_api_base::media::PLAIN_TEXT;

    pub use orch_api_base::media::SNIFF_BYTES;

    pub use orch_api_base::media::UNKNOWN;

    pub use orch_api_base::media::is_text;

    pub use orch_api_base::media::is_text_by_name;

    pub use orch_api_base::media::media_type;
    #[cfg(test)]
    mod tests {
        #[cfg(test)]
        #[test]
        fn a_name_decides_the_type_and_the_first_bytes_decide_where_the_name_does_not() {
            orch_api_base::media::tests::__api_run_a_name_decides_the_type_and_the_first_bytes_decide_where_the_name_does_not()
        }
        #[cfg(test)]
        #[test]
        fn content_is_text_when_it_holds_no_nul_and_reads_as_utf8() {
            orch_api_base::media::tests::__api_run_content_is_text_when_it_holds_no_nul_and_reads_as_utf8()
        }
        #[cfg(test)]
        #[test]
        fn a_name_for_a_path_has_no_separator_and_no_leading_dot() {
            orch_api_base::media::tests::__api_run_a_name_for_a_path_has_no_separator_and_no_leading_dot()
        }
        #[cfg(test)]
        #[test]
        fn active_content_is_not_shown_inline() {
            orch_api_base::media::tests::__api_run_active_content_is_not_shown_inline()
        }
    }
}

pub mod middleware {

    pub mod authentication {

        pub use orch_api_base::middleware::authentication::ACCESS_TOKEN_PREFIX;

        pub use orch_api_base::middleware::authentication::SESSION_PREFIX;

        pub use orch_api_base::middleware::authentication::API_TOKEN_PREFIX;

        pub use orch_api_base::middleware::authentication::Via;

        pub use orch_api_base::middleware::authentication::Authed;

        pub use orch_api_base::middleware::authentication::Caller;

        pub use orch_api_base::middleware::authentication::token_hash;

        pub use orch_api_base::middleware::authentication::new_token;

        pub use orch_api_base::middleware::authentication::bearer;

        pub use orch_api_base::middleware::authentication::cookie;

        pub use orch_api_base::middleware::authentication::on_surface;

        pub use orch_api_base::middleware::authentication::authenticate_on;

        pub use orch_api_base::middleware::authentication::authenticate;

        pub use orch_api_base::middleware::authentication::NativeReadIdentity;
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn surface_headers_reject_foreign_kinds_before_public_and_cookie_shortcuts() {
                orch_api_base::middleware::authentication::tests::__api_run_surface_headers_reject_foreign_kinds_before_public_and_cookie_shortcuts()
            }
            #[cfg(test)]
            #[test]
            fn a_cookie_is_found_among_others() {
                orch_api_base::middleware::authentication::tests::__api_run_a_cookie_is_found_among_others()
            }
            #[cfg(test)]
            #[test]
            fn tokens_are_long_random_and_told_apart_by_their_prefix() {
                orch_api_base::middleware::authentication::tests::__api_run_tokens_are_long_random_and_told_apart_by_their_prefix()
            }
        }
    }

    pub mod authorization {

        pub use orch_api_base::middleware::authorization::VIEW;

        pub use orch_api_base::middleware::authorization::MANAGE;

        pub use orch_api_base::middleware::authorization::exact;

        pub use orch_api_base::middleware::authorization::caller_of;

        pub use orch_api_base::middleware::authorization::principal;

        pub use orch_api_base::middleware::authorization::engine_problem;

        pub use orch_api_base::middleware::authorization::resource_kind_of;

        pub use orch_api_base::middleware::authorization::require_on_path;

        pub use orch_api_base::middleware::authorization::require;

        pub use orch_api_base::middleware::authorization::kept_out_problem;

        pub use orch_api_base::middleware::authorization::kept_out;

        pub use orch_api_base::middleware::authorization::require_resource;

        pub use orch_api_base::middleware::authorization::require_document;
        #[cfg(test)]
        mod projection_tests {
            #[cfg(test)]
            #[test]
            fn limited_problem_keeps_code_and_retry_without_named_payload() {
                orch_api_base::middleware::authorization::projection_tests::__api_run_limited_problem_keeps_code_and_retry_without_named_payload()
            }
            #[cfg(test)]
            #[test]
            fn limited_problem_preserves_trusted_retry_without_trusting_a_public_lookalike() {
                orch_api_base::middleware::authorization::projection_tests::__api_run_limited_problem_preserves_trusted_retry_without_trusting_a_public_lookalike()
            }
        }

        pub mod refusals {

            pub use orch_api_base::middleware::authorization::refusals::user_ref;

            pub use orch_api_base::middleware::authorization::refusals::user_refs;

            pub use orch_api_base::middleware::authorization::refusals::node_kind;

            pub use orch_api_base::middleware::authorization::refusals::named_node;

            pub use orch_api_base::middleware::authorization::refusals::missing_access;

            pub use orch_api_base::middleware::authorization::refusals::denied;

            pub use orch_api_base::middleware::authorization::refusals::problem;

            pub use orch_api_base::middleware::authorization::refusals::settle;

            pub use orch_api_base::middleware::authorization::refusals::change_failed;

            pub use orch_api_base::middleware::authorization::refusals::external_forbidden;

            pub use orch_api_base::middleware::authorization::refusals::system_admin_only;
        }
    }

    pub mod idempotency {
        #[cfg(test)]
        pub(crate) mod commands {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[tokio::test]
                async fn each_native_creating_command_is_atomic_with_its_exact_data_only_reference()
                -> ::orch_api_base::middleware::idempotency::commands::tests::Proof
                {
                    orch_api_base::middleware::idempotency::commands::tests::__api_run_each_native_creating_command_is_atomic_with_its_exact_data_only_reference().await
                }
                #[cfg(test)]
                #[tokio::test]
                async fn a_native_preparation_of_the_same_kind_does_not_claim_the_later_creation()
                -> ::orch_api_base::middleware::idempotency::commands::tests::Proof
                {
                    orch_api_base::middleware::idempotency::commands::tests::__api_run_a_native_preparation_of_the_same_kind_does_not_claim_the_later_creation().await
                }
            }
        }
        #[cfg(test)]
        pub(crate) mod memory {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn only_publishing_keeps_the_native_memory_entry() {
                    orch_api_base::middleware::idempotency::memory::tests::__api_run_only_publishing_keeps_the_native_memory_entry()
                }
            }
        }
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[tokio::test]
            async fn partial_native_progress_resumes_under_the_same_key_and_a_completed_outcome_stays_final()
            -> Result<(), Box<dyn ::std::error::Error + Send + Sync>> {
                orch_api_base::middleware::idempotency::tests::__api_run_partial_native_progress_resumes_under_the_same_key_and_a_completed_outcome_stays_final().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn a_completed_creation_never_installs_the_original_writer_request()
            -> Result<(), Box<dyn ::std::error::Error + Send + Sync>> {
                orch_api_base::middleware::idempotency::tests::__api_run_a_completed_creation_never_installs_the_original_writer_request().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn an_identity_free_consumer_without_its_observer_fails_closed_instead_of_running_twice()
            -> Result<(), Box<dyn ::std::error::Error + Send + Sync>> {
                orch_api_base::middleware::idempotency::tests::__api_run_an_identity_free_consumer_without_its_observer_fails_closed_instead_of_running_twice().await
            }
        }
    }
    #[cfg(test)]
    pub(crate) mod inspection {
        #[cfg(test)]
        mod snapshot_tests {
            #[cfg(test)]
            #[test]
            fn successful_snapshot_keeps_exact_deletion_and_read_scope_without_auto_upgrade() {
                orch_api_base::middleware::inspection::snapshot_tests::__api_run_successful_snapshot_keeps_exact_deletion_and_read_scope_without_auto_upgrade()
            }
        }
    }

    pub mod requests {

        pub use orch_api_base::middleware::requests::ApiError;

        pub use orch_api_base::middleware::requests::ApiResult;
        #[cfg(test)]
        mod words {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn extras_cannot_replace_contract_fields_or_publish_internal_diagnostics() {
                    orch_api_base::middleware::requests::words::tests::__api_run_extras_cannot_replace_contract_fields_or_publish_internal_diagnostics()
                }
                #[cfg(test)]
                #[test]
                fn field_diagnostics_are_withheld_without_losing_the_field_or_useful_explanation() {
                    orch_api_base::middleware::requests::words::tests::__api_run_field_diagnostics_are_withheld_without_losing_the_field_or_useful_explanation()
                }
            }
        }
        #[cfg(test)]
        mod check_tests {
            #[cfg(test)]
            #[test]
            fn a_value_the_schemas_check_refuses_is_the_requests_fault_at_its_field() {
                orch_api_base::middleware::requests::check_tests::__api_run_a_value_the_schemas_check_refuses_is_the_requests_fault_at_its_field()
            }
            #[cfg(test)]
            #[test]
            fn a_known_native_source_refusal_is_an_authorization_answer_without_an_internal_error()
            {
                orch_api_base::middleware::requests::check_tests::__api_run_a_known_native_source_refusal_is_an_authorization_answer_without_an_internal_error()
            }
            #[cfg(test)]
            #[test]
            fn only_a_typed_database_conflict_marks_an_effect_free_read_for_retry() {
                orch_api_base::middleware::requests::check_tests::__api_run_only_a_typed_database_conflict_marks_an_effect_free_read_for_retry()
            }
            #[cfg(test)]
            #[test]
            fn an_engine_commit_that_lost_a_race_is_the_callers_to_send_again_and_nothing_else_is()
            {
                orch_api_base::middleware::requests::check_tests::__api_run_an_engine_commit_that_lost_a_race_is_the_callers_to_send_again_and_nothing_else_is()
            }
        }
    }

    pub mod transaction_guard {

        pub use orch_api_base::middleware::transaction_guard::LEDGER;

        pub use orch_api_base::middleware::transaction_guard::Boundary;

        pub use orch_api_base::middleware::transaction_guard::boundary_of;

        pub use orch_api_base::middleware::transaction_guard::Guard;

        pub use orch_api_base::middleware::transaction_guard::begin;

        pub use orch_api_base::middleware::transaction_guard::check;
        #[cfg(test)]
        mod deadline_tests {
            #[cfg(test)]
            #[tokio::test]
            async fn original_raw_and_accepted_key_deadlines_survive_a_real_native_preparation_wait()
             {
                orch_api_base::middleware::transaction_guard::deadline_tests::__api_run_original_raw_and_accepted_key_deadlines_survive_a_real_native_preparation_wait().await
            }
        }
    }
}

pub mod port_tools {

    pub use orch_api_base::port_tools::DECISION_WAIT;

    pub use orch_api_base::port_tools::install;

    pub use orch_api_base::port_tools::Tools;

    pub use orch_api_base::port_tools::tell_late;
    #[cfg(test)]
    mod budget_tests {
        #[cfg(test)]
        #[test]
        fn supplied_marks_and_multibyte_text_count_before_a_change_is_allowed() {
            orch_api_base::port_tools::budget_tests::__api_run_supplied_marks_and_multibyte_text_count_before_a_change_is_allowed()
        }
    }
}

pub mod ports_browser {

    pub use orch_api_base::ports_browser::ENTRY_COOKIE;

    pub use orch_api_base::ports_browser::ENTER_PATH;

    pub use orch_api_base::ports_browser::LINK_TOKEN;

    pub use orch_api_base::ports_browser::dispatch;
    #[cfg(test)]
    mod tests {
        #[cfg(test)]
        #[test]
        fn orch_s_cookie_and_the_link_token_never_reach_the_program() {
            orch_api_base::ports_browser::tests::__api_run_orch_s_cookie_and_the_link_token_never_reach_the_program()
        }
        #[cfg(test)]
        #[test]
        fn a_caller_s_own_identity_headers_are_taken_out_in_every_case_and_copy() {
            orch_api_base::ports_browser::tests::__api_run_a_caller_s_own_identity_headers_are_taken_out_in_every_case_and_copy()
        }
        #[cfg(test)]
        #[test]
        fn a_display_name_travels_as_percent_text() {
            orch_api_base::ports_browser::tests::__api_run_a_display_name_travels_as_percent_text()
        }
    }
}

pub mod ports_runtime {

    pub use orch_api_base::ports_runtime::USE_SECONDS;

    pub use orch_api_base::ports_runtime::BrowserEntry;

    pub use orch_api_base::ports_runtime::BROWSER_ENTRY_HOURS;

    pub use orch_api_base::ports_runtime::PortRuntime;

    pub use orch_api_base::ports_runtime::may_end_admission;

    pub use orch_api_base::ports_runtime::close_ended;

    pub use orch_api_base::ports_runtime::start;
    #[cfg(test)]
    mod surface_tests {
        #[cfg(test)]
        #[tokio::test]
        async fn surface_port_use_refusal_preserves_the_same_proof_and_origin() {
            orch_api_base::ports_runtime::surface_tests::__api_run_surface_port_use_refusal_preserves_the_same_proof_and_origin().await
        }
    }
}

pub mod previews {

    pub use orch_api_base::previews::serve;

    pub use orch_api_base::previews::source_limit;

    pub use orch_api_base::previews::unavailable;
    #[cfg(test)]
    mod tests {
        #[cfg(test)]
        #[test]
        fn text_cards_share_the_reference_card_bound_even_for_one_very_long_line() {
            orch_api_base::previews::tests::__api_run_text_cards_share_the_reference_card_bound_even_for_one_very_long_line()
        }
        #[cfg(test)]
        #[test]
        fn active_original_types_are_inert_downloads_from_the_shared_file_server() {
            orch_api_base::previews::tests::__api_run_active_original_types_are_inert_downloads_from_the_shared_file_server()
        }
        #[cfg(test)]
        #[test]
        fn previews_are_bounded_static_images_and_keep_transparency() {
            orch_api_base::previews::tests::__api_run_previews_are_bounded_static_images_and_keep_transparency()
        }
        #[cfg(test)]
        #[test]
        fn an_image_over_the_pixel_budget_is_refused_before_decode() {
            orch_api_base::previews::tests::__api_run_an_image_over_the_pixel_budget_is_refused_before_decode()
        }
        #[cfg(test)]
        #[test]
        fn the_static_preview_applies_the_original_exif_orientation() {
            orch_api_base::previews::tests::__api_run_the_static_preview_applies_the_original_exif_orientation()
        }
        #[cfg(test)]
        #[test]
        fn an_animated_image_produces_only_its_first_frame_as_a_png() {
            orch_api_base::previews::tests::__api_run_an_animated_image_produces_only_its_first_frame_as_a_png()
        }
        #[cfg(test)]
        #[test]
        fn svg_is_rasterized_without_resolving_external_images_or_running_scripts() {
            orch_api_base::previews::tests::__api_run_svg_is_rasterized_without_resolving_external_images_or_running_scripts()
        }
        #[cfg(test)]
        #[test]
        fn the_cache_is_byte_bounded_and_discards_the_least_recently_used_preview() {
            orch_api_base::previews::tests::__api_run_the_cache_is_byte_bounded_and_discards_the_least_recently_used_preview()
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_reader_leaving_does_not_repeat_the_same_original_read_or_lose_its_cached_preview()
         {
            orch_api_base::previews::tests::__api_run_a_reader_leaving_does_not_repeat_the_same_original_read_or_lose_its_cached_preview().await
        }
    }
}

pub mod programs {

    pub use orch_api_base::programs::executor_info;

    pub use orch_api_base::programs::executor_info_of;

    pub use orch_api_base::programs::machine_executors;

    pub use orch_api_base::programs::executors;

    pub use orch_api_base::programs::carry_of;

    pub use orch_api_base::programs::check_catalogue;

    pub use orch_api_base::programs::program_info;

    pub use orch_api_base::programs::executor_ref;

    pub use orch_api_base::programs::harness_name;

    pub use orch_api_base::programs::named_by;

    pub use orch_api_base::programs::shipped_name;

    pub use orch_api_base::programs::tool_set_info;
    #[cfg(test)]
    mod tests {
        #[cfg(test)]
        #[test]
        fn the_offers_of_an_executor_are_its_sets_namespaced_with_the_old_word_as_their_alias() {
            orch_api_base::programs::tests::__api_run_the_offers_of_an_executor_are_its_sets_namespaced_with_the_old_word_as_their_alias()
        }
        #[cfg(test)]
        #[test]
        fn an_external_executors_report_carries_localized_tool_set_text_without_identifier_fallback()
         {
            orch_api_base::programs::tests::__api_run_an_external_executors_report_carries_localized_tool_set_text_without_identifier_fallback()
        }
        #[cfg(test)]
        #[test]
        fn the_check_of_a_program_is_titled_case_by_case_and_feature_by_feature() {
            orch_api_base::programs::tests::__api_run_the_check_of_a_program_is_titled_case_by_case_and_feature_by_feature()
        }
    }
}

pub mod realtime {

    pub use orch_api_base::realtime::SOCKET_ROUTE;

    pub use orch_api_base::realtime::EPOCH_EVERY;

    pub use orch_api_base::realtime::FANOUT_GAP;

    pub use orch_api_base::realtime::STREAM_GAP;

    pub use orch_api_base::realtime::STREAMING_WINDOW;

    pub use orch_api_base::realtime::TICK;

    pub use orch_api_base::realtime::CLOSE_GRACE;

    pub use orch_api_base::realtime::Tuning;

    pub use orch_api_base::realtime::Fail;

    pub use orch_api_base::realtime::Source;

    pub use orch_api_base::realtime::Realtime;
    #[cfg(test)]
    pub(crate) mod background_read_tests {
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn http_session_expiry_after_native_preparation_withholds_the_roster()
        -> ::orch_api_base::realtime::background_read_tests::Proof {
            orch_api_base::realtime::background_read_tests::__api_run_http_session_expiry_after_native_preparation_withholds_the_roster().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn http_legacy_key_expiry_after_native_preparation_withholds_the_roster()
        -> ::orch_api_base::realtime::background_read_tests::Proof {
            orch_api_base::realtime::background_read_tests::__api_run_http_legacy_key_expiry_after_native_preparation_withholds_the_roster().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn http_scoped_key_expiry_after_native_preparation_withholds_the_roster()
        -> ::orch_api_base::realtime::background_read_tests::Proof {
            orch_api_base::realtime::background_read_tests::__api_run_http_scoped_key_expiry_after_native_preparation_withholds_the_roster().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn http_nonexpiring_key_remains_valid_after_native_preparation()
        -> ::orch_api_base::realtime::background_read_tests::Proof {
            orch_api_base::realtime::background_read_tests::__api_run_http_nonexpiring_key_remains_valid_after_native_preparation().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn http_second_page_reprojects_an_expired_optional_source_without_native_recapture()
        -> ::orch_api_base::realtime::background_read_tests::Proof {
            orch_api_base::realtime::background_read_tests::__api_run_http_second_page_reprojects_an_expired_optional_source_without_native_recapture().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn http_native_wait_uses_the_actual_session_mode_without_borrowing_another_session()
        -> ::orch_api_base::realtime::background_read_tests::Proof {
            orch_api_base::realtime::background_read_tests::__api_run_http_native_wait_uses_the_actual_session_mode_without_borrowing_another_session().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn socket_initial_seed_reprojects_optional_source_expiry_without_an_epoch_change()
        -> ::orch_api_base::realtime::background_read_tests::Proof {
            orch_api_base::realtime::background_read_tests::__api_run_socket_initial_seed_reprojects_optional_source_expiry_without_an_epoch_change().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn socket_broadcast_reprojects_optional_source_expiry_without_an_epoch_change()
        -> ::orch_api_base::realtime::background_read_tests::Proof {
            orch_api_base::realtime::background_read_tests::__api_run_socket_broadcast_reprojects_optional_source_expiry_without_an_epoch_change().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn socket_raw_session_expiry_withholds_a_projected_roster_and_closes()
        -> ::orch_api_base::realtime::background_read_tests::Proof {
            orch_api_base::realtime::background_read_tests::__api_run_socket_raw_session_expiry_withholds_a_projected_roster_and_closes().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn http_background_uses_its_actual_second_step_hold_after_source_preparation()
        -> ::orch_api_base::realtime::background_read_tests::Proof {
            orch_api_base::realtime::background_read_tests::__api_run_http_background_uses_its_actual_second_step_hold_after_source_preparation().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn socket_background_closes_its_feed_on_actual_second_step_loss_without_borrowing_peer_proof()
        -> ::orch_api_base::realtime::background_read_tests::Proof {
            orch_api_base::realtime::background_read_tests::__api_run_socket_background_closes_its_feed_on_actual_second_step_loss_without_borrowing_peer_proof().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn indirect_role_expiry_reprojects_provider_directories_from_the_native_holder_stamp()
        -> ::orch_api_base::realtime::background_read_tests::Proof {
            orch_api_base::realtime::background_read_tests::__api_run_indirect_role_expiry_reprojects_provider_directories_from_the_native_holder_stamp().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn native_background_wrapper_preserves_scoped_admission_and_current_key_revocation()
        -> ::orch_api_base::realtime::background_read_tests::Proof {
            orch_api_base::realtime::background_read_tests::__api_run_native_background_wrapper_preserves_scoped_admission_and_current_key_revocation().await
        }
    }

    pub mod browsers {

        pub use orch_api_base::realtime::browsers::Sockets;
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn surface_ticket_foreign_redemption_cannot_consume_either_origin() {
                orch_api_base::realtime::browsers::tests::__api_run_surface_ticket_foreign_redemption_cannot_consume_either_origin()
            }
            #[cfg(test)]
            #[test]
            fn file_navigation_and_workspace_chooser_paths_need_the_files_door() {
                orch_api_base::realtime::browsers::tests::__api_run_file_navigation_and_workspace_chooser_paths_need_the_files_door()
            }
            #[cfg(test)]
            #[test]
            fn a_sitting_keeps_only_the_host_of_a_visited_address() {
                orch_api_base::realtime::browsers::tests::__api_run_a_sitting_keeps_only_the_host_of_a_visited_address()
            }
        }
    }

    pub mod deltas {

        pub use orch_api_base::realtime::deltas::Tag;

        pub use orch_api_base::realtime::deltas::Held;

        pub use orch_api_base::realtime::deltas::Chunk;

        pub use orch_api_base::realtime::deltas::Next;

        pub use orch_api_base::realtime::deltas::Outbox;

        pub use orch_api_base::realtime::deltas::Sink;
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn deltas_that_wait_for_a_slow_reader_become_one_larger_delta_and_lose_nothing() {
                orch_api_base::realtime::deltas::tests::__api_run_deltas_that_wait_for_a_slow_reader_become_one_larger_delta_and_lose_nothing()
            }
            #[cfg(test)]
            #[test]
            fn a_reader_who_takes_each_frame_as_it_comes_is_sent_each_delta_as_it_grew() {
                orch_api_base::realtime::deltas::tests::__api_run_a_reader_who_takes_each_frame_as_it_comes_is_sent_each_delta_as_it_grew()
            }
            #[cfg(test)]
            #[test]
            fn the_deltas_of_two_items_are_not_mixed() {
                orch_api_base::realtime::deltas::tests::__api_run_the_deltas_of_two_items_are_not_mixed()
            }
            #[cfg(test)]
            #[test]
            fn a_delta_that_does_not_continue_the_one_that_waits_goes_behind_it() {
                orch_api_base::realtime::deltas::tests::__api_run_a_delta_that_does_not_continue_the_one_that_waits_goes_behind_it()
            }
            #[cfg(test)]
            #[test]
            fn a_row_that_holds_all_that_a_waiting_delta_says_makes_the_delta_pointless() {
                orch_api_base::realtime::deltas::tests::__api_run_a_row_that_holds_all_that_a_waiting_delta_says_makes_the_delta_pointless()
            }
            #[cfg(test)]
            #[test]
            fn a_row_that_holds_only_the_start_of_a_waiting_delta_goes_behind_it_and_what_grows_next_is_a_delta_of_its_own()
             {
                orch_api_base::realtime::deltas::tests::__api_run_a_row_that_holds_only_the_start_of_a_waiting_delta_goes_behind_it_and_what_grows_next_is_a_delta_of_its_own()
            }
            #[cfg(test)]
            #[test]
            fn a_row_older_than_a_waiting_delta_leaves_it_in_place_to_grow() {
                orch_api_base::realtime::deltas::tests::__api_run_a_row_older_than_a_waiting_delta_leaves_it_in_place_to_grow()
            }
            #[cfg(test)]
            #[test]
            fn rows_are_never_dropped_and_leave_in_the_order_they_were_queued() {
                orch_api_base::realtime::deltas::tests::__api_run_rows_are_never_dropped_and_leave_in_the_order_they_were_queued()
            }
            #[cfg(test)]
            #[test]
            fn an_answer_to_a_ping_leaves_before_what_waits() {
                orch_api_base::realtime::deltas::tests::__api_run_an_answer_to_a_ping_leaves_before_what_waits()
            }
            #[cfg(test)]
            #[test]
            fn what_an_ended_subscription_left_in_the_queue_is_taken_out_and_the_others_are_not() {
                orch_api_base::realtime::deltas::tests::__api_run_what_an_ended_subscription_left_in_the_queue_is_taken_out_and_the_others_are_not()
            }
            #[cfg(test)]
            #[test]
            fn past_the_limit_deltas_and_events_are_shed_and_each_subscription_is_told_to_read_again_once()
             {
                orch_api_base::realtime::deltas::tests::__api_run_past_the_limit_deltas_and_events_are_shed_and_each_subscription_is_told_to_read_again_once()
            }
            #[cfg(test)]
            #[test]
            fn the_limit_is_for_what_waits_and_a_reader_that_keeps_taking_frames_is_never_over_it()
            {
                orch_api_base::realtime::deltas::tests::__api_run_the_limit_is_for_what_waits_and_a_reader_that_keeps_taking_frames_is_never_over_it()
            }
            #[cfg(test)]
            #[test]
            fn a_connection_that_takes_nothing_is_closed_with_4408_after_the_time_and_not_before() {
                orch_api_base::realtime::deltas::tests::__api_run_a_connection_that_takes_nothing_is_closed_with_4408_after_the_time_and_not_before()
            }
            #[cfg(test)]
            #[test]
            fn a_frame_in_the_sockets_hands_counts_as_waiting() {
                orch_api_base::realtime::deltas::tests::__api_run_a_frame_in_the_sockets_hands_counts_as_waiting()
            }
            #[cfg(test)]
            #[test]
            fn frames_queued_after_the_connection_was_closed_are_not_kept() {
                orch_api_base::realtime::deltas::tests::__api_run_frames_queued_after_the_connection_was_closed_are_not_kept()
            }
            #[cfg(test)]
            #[tokio::test]
            async fn the_writer_waits_for_a_frame_and_is_woken_by_it() {
                orch_api_base::realtime::deltas::tests::__api_run_the_writer_waits_for_a_frame_and_is_woken_by_it().await
            }
        }
    }

    pub mod displays {

        pub use orch_api_base::realtime::displays::Sockets;
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn surface_ticket_foreign_redemption_cannot_consume_either_origin() {
                orch_api_base::realtime::displays::tests::__api_run_surface_ticket_foreign_redemption_cannot_consume_either_origin()
            }
            #[cfg(test)]
            #[test]
            fn a_viewer_is_asked_for_no_password_even_when_its_messages_are_split() {
                orch_api_base::realtime::displays::tests::__api_run_a_viewer_is_asked_for_no_password_even_when_its_messages_are_split()
            }
            #[cfg(test)]
            #[test]
            fn a_false_security_choice_ends_the_connection_instead_of_becoming_input() {
                orch_api_base::realtime::displays::tests::__api_run_a_false_security_choice_ends_the_connection_instead_of_becoming_input()
            }
        }
    }

    pub mod ephemeral {

        pub use orch_api_base::realtime::ephemeral::STOP_GRACE;

        pub use orch_api_base::realtime::ephemeral::COORDINATOR;

        pub use orch_api_base::realtime::ephemeral::forward_report;
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[tokio::test]
            async fn watching_a_thousand_machines_among_a_million_reads_only_indexed_batches() {
                orch_api_base::realtime::ephemeral::tests::__api_run_watching_a_thousand_machines_among_a_million_reads_only_indexed_batches().await
            }
            #[cfg(test)]
            #[test]
            fn a_departed_watch_stays_for_ten_seconds_and_a_return_cancels_its_departure() {
                orch_api_base::realtime::ephemeral::tests::__api_run_a_departed_watch_stays_for_ten_seconds_and_a_return_cancels_its_departure()
            }
            #[cfg(test)]
            #[test]
            fn ten_lists_in_one_second_leave_only_the_last_one_to_be_sent() {
                orch_api_base::realtime::ephemeral::tests::__api_run_ten_lists_in_one_second_leave_only_the_last_one_to_be_sent()
            }
            #[cfg(test)]
            #[test]
            fn partial_directory_invalidation_retains_rows_but_cannot_reuse_a_snapshot_cursor() {
                orch_api_base::realtime::ephemeral::tests::__api_run_partial_directory_invalidation_retains_rows_but_cannot_reuse_a_snapshot_cursor()
            }
            #[cfg(test)]
            #[test]
            fn a_removed_file_cancels_an_earlier_change_and_a_recreated_file_cancels_its_removal() {
                orch_api_base::realtime::ephemeral::tests::__api_run_a_removed_file_cancels_an_earlier_change_and_a_recreated_file_cancels_its_removal()
            }
        }
    }
    #[cfg(test)]
    mod epoch_handoff_tests {
        #[cfg(test)]
        mod native_feed_keys {
            #[cfg(test)]
            #[tokio::test]
            async fn raw_manage_key_reads_the_same_segment_over_http_and_live_delivery_then_revocation_stops_it()
            -> ::orch_api_base::realtime::epoch_handoff_tests::Proof {
                orch_api_base::realtime::epoch_handoff_tests::native_feed_keys::__api_run_raw_manage_key_reads_the_same_segment_over_http_and_live_delivery_then_revocation_stops_it().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn raw_scope_does_not_supply_ordinary_management_or_admit_a_foreign_segment()
            -> ::orch_api_base::realtime::epoch_handoff_tests::Proof {
                orch_api_base::realtime::epoch_handoff_tests::native_feed_keys::__api_run_raw_scope_does_not_supply_ordinary_management_or_admit_a_foreign_segment().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn background_read_uses_the_same_provider_directory_scope_for_http_and_delivery()
            -> ::orch_api_base::realtime::epoch_handoff_tests::Proof {
                orch_api_base::realtime::epoch_handoff_tests::native_feed_keys::__api_run_background_read_uses_the_same_provider_directory_scope_for_http_and_delivery().await
            }
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn notice_first_preserves_the_native_enrollment_status_and_purges_old_inbox_rows()
        -> ::orch_api_base::realtime::epoch_handoff_tests::Proof {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_notice_first_preserves_the_native_enrollment_status_and_purges_old_inbox_rows().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn authority_recheck_first_preserves_the_same_native_issuer_notice()
        -> ::orch_api_base::realtime::epoch_handoff_tests::Proof {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_authority_recheck_first_preserves_the_same_native_issuer_notice().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn generic_personal_state_stays_current_after_repeated_inbox_resets_and_never_revives_a_closed_reader()
        -> ::orch_api_base::realtime::epoch_handoff_tests::Proof {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_generic_personal_state_stays_current_after_repeated_inbox_resets_and_never_revives_a_closed_reader().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_revoked_exact_session_cannot_receive_personal_news_while_its_other_session_still_can()
        -> ::orch_api_base::realtime::epoch_handoff_tests::Proof {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_a_revoked_exact_session_cannot_receive_personal_news_while_its_other_session_still_can().await
        }
        #[cfg(test)]
        #[test]
        fn native_row_failure_diagnostics_keep_dynamic_keys_and_values_opaque() {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_native_row_failure_diagnostics_keep_dynamic_keys_and_values_opaque()
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn deleting_an_environment_during_initial_projection_closes_only_that_acknowledged_reader()
        -> ::orch_api_base::realtime::epoch_handoff_tests::Proof {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_deleting_an_environment_during_initial_projection_closes_only_that_acknowledged_reader().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn an_invalid_projection_on_a_live_environment_remains_an_error_not_a_deleted_reader()
        -> ::orch_api_base::realtime::epoch_handoff_tests::Proof {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_an_invalid_projection_on_a_live_environment_remains_an_error_not_a_deleted_reader().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn native_resource_manage_withdrawal_purges_private_progress_then_last_right_withdrawal_closes_the_reader()
        -> ::orch_api_base::realtime::epoch_handoff_tests::Proof {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_native_resource_manage_withdrawal_purges_private_progress_then_last_right_withdrawal_closes_the_reader().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_purged_unsent_native_resource_replacement_is_reprojected_but_private_progress_does_not_break_quietness()
        -> ::orch_api_base::realtime::epoch_handoff_tests::Proof {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_a_purged_unsent_native_resource_replacement_is_reprojected_but_private_progress_does_not_break_quietness().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_changed_personal_projection_cut_reprojects_the_notice_before_publication()
        -> ::orch_api_base::realtime::epoch_handoff_tests::Proof {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_a_changed_personal_projection_cut_reprojects_the_notice_before_publication().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_credential_deadline_crossed_during_paused_inbox_projection_prevents_publication()
        -> ::orch_api_base::realtime::epoch_handoff_tests::Proof {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_a_credential_deadline_crossed_during_paused_inbox_projection_prevents_publication().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_manage_withdrawal_after_native_resource_projection_cannot_publish_the_old_rich_access()
        -> ::orch_api_base::realtime::epoch_handoff_tests::Proof {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_a_manage_withdrawal_after_native_resource_projection_cannot_publish_the_old_rich_access().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn purging_one_resource_generation_keeps_the_other_views_queued_replacement_and_fingerprint()
        -> ::orch_api_base::realtime::epoch_handoff_tests::Proof {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_purging_one_resource_generation_keeps_the_other_views_queued_replacement_and_fingerprint().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn metadata_hints_use_existing_personal_readers_and_keep_exact_session_fences()
        -> ::orch_api_base::realtime::epoch_handoff_tests::Proof {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_metadata_hints_use_existing_personal_readers_and_keep_exact_session_fences().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_nodes_machine_feed_keeps_host_view_until_the_physical_machine_is_deleted()
        -> ::orch_api_base::realtime::epoch_handoff_tests::Proof {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_a_nodes_machine_feed_keeps_host_view_until_the_physical_machine_is_deleted().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn an_existing_scoped_key_authenticates_realtime_as_that_key_and_revocation_ends_it()
        -> ::orch_api_base::realtime::epoch_handoff_tests::Proof {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_an_existing_scoped_key_authenticates_realtime_as_that_key_and_revocation_ends_it().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn an_owner_read_key_observes_its_thread_but_does_not_inherit_raw_protocol_management()
        -> ::orch_api_base::realtime::epoch_handoff_tests::Proof {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_an_owner_read_key_observes_its_thread_but_does_not_inherit_raw_protocol_management().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn topic_sync_uses_the_same_selection_for_scoped_http_and_socket_child_rows()
        -> ::orch_api_base::realtime::epoch_handoff_tests::Proof {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_topic_sync_uses_the_same_selection_for_scoped_http_and_socket_child_rows().await
        }
    }
    #[cfg(test)]
    pub(crate) mod history_read_tests {
        #[cfg(test)]
        #[tokio::test]
        async fn document_changes_held_projection_rechecks_actual_sources_before_publication()
        -> ::orch_api_base::realtime::history_read_tests::Proof {
            orch_api_base::realtime::history_read_tests::__api_run_document_changes_held_projection_rechecks_actual_sources_before_publication().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn document_list_held_page_rechecks_its_actual_named_sources()
        -> ::orch_api_base::realtime::history_read_tests::Proof {
            orch_api_base::realtime::history_read_tests::__api_run_document_list_held_page_rechecks_its_actual_named_sources().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn legacy_document_reply_rechecks_held_sources_after_the_same_key_becomes_modern()
        -> ::orch_api_base::realtime::history_read_tests::Proof {
            orch_api_base::realtime::history_read_tests::__api_run_legacy_document_reply_rechecks_held_sources_after_the_same_key_becomes_modern().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn an_open_file_cannot_combine_old_scope_placement_with_new_ordinary_document_access()
        -> ::orch_api_base::realtime::history_read_tests::Proof {
            orch_api_base::realtime::history_read_tests::__api_run_an_open_file_cannot_combine_old_scope_placement_with_new_ordinary_document_access().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn scoped_history_segment_shrink_after_snapshot_refuses_stale_content()
        -> ::orch_api_base::realtime::history_read_tests::Proof {
            orch_api_base::realtime::history_read_tests::__api_run_scoped_history_segment_shrink_after_snapshot_refuses_stale_content().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn scoped_history_turn_shrink_after_snapshot_refuses_stale_content()
        -> ::orch_api_base::realtime::history_read_tests::Proof {
            orch_api_base::realtime::history_read_tests::__api_run_scoped_history_turn_shrink_after_snapshot_refuses_stale_content().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn http_sync_head_count_and_rows_stay_on_one_cut_across_a_terminal_commit()
        -> ::orch_api_base::realtime::history_read_tests::Proof {
            orch_api_base::realtime::history_read_tests::__api_run_http_sync_head_count_and_rows_stay_on_one_cut_across_a_terminal_commit().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn socket_rows_and_personalized_thread_do_not_mix_two_commits()
        -> ::orch_api_base::realtime::history_read_tests::Proof {
            orch_api_base::realtime::history_read_tests::__api_run_socket_rows_and_personalized_thread_do_not_mix_two_commits().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn a_late_wake_carries_every_changed_row_through_the_actual_snapshot_head()
        -> ::orch_api_base::realtime::history_read_tests::Proof {
            orch_api_base::realtime::history_read_tests::__api_run_a_late_wake_carries_every_changed_row_through_the_actual_snapshot_head().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn http_incremental_budget_and_rows_use_the_same_snapshot_as_its_head()
        -> ::orch_api_base::realtime::history_read_tests::Proof {
            orch_api_base::realtime::history_read_tests::__api_run_http_incremental_budget_and_rows_use_the_same_snapshot_as_its_head().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn opening_outline_and_conversation_keep_the_same_cut_while_the_native_turn_ends()
        -> ::orch_api_base::realtime::history_read_tests::Proof {
            orch_api_base::realtime::history_read_tests::__api_run_opening_outline_and_conversation_keep_the_same_cut_while_the_native_turn_ends().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn socket_request_cards_share_the_row_cut_and_never_share_another_readers_authority()
        -> ::orch_api_base::realtime::history_read_tests::Proof {
            orch_api_base::realtime::history_read_tests::__api_run_socket_request_cards_share_the_row_cut_and_never_share_another_readers_authority().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn scoped_raw_thread_output_rechecks_same_key_after_open_before_bytes()
        -> ::orch_api_base::realtime::history_read_tests::Proof {
            orch_api_base::realtime::history_read_tests::__api_run_scoped_raw_thread_output_rechecks_same_key_after_open_before_bytes().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn scoped_raw_thread_blob_rechecks_same_key_after_open_before_bytes()
        -> ::orch_api_base::realtime::history_read_tests::Proof {
            orch_api_base::realtime::history_read_tests::__api_run_scoped_raw_thread_blob_rechecks_same_key_after_open_before_bytes().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn browser_close_name_source_and_projection_never_mix_p_with_q()
        -> ::orch_api_base::realtime::history_read_tests::Proof {
            orch_api_base::realtime::history_read_tests::__api_run_browser_close_name_source_and_projection_never_mix_p_with_q().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn display_close_name_source_and_projection_never_mix_p_with_q()
        -> ::orch_api_base::realtime::history_read_tests::Proof {
            orch_api_base::realtime::history_read_tests::__api_run_display_close_name_source_and_projection_never_mix_p_with_q().await
        }
    }
    #[cfg(test)]
    mod origin_read_tests {
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn browser_login_checks_its_actual_session_deadline_after_identity_wait()
        -> ::orch_api_base::realtime::origin_read_tests::Proof {
            orch_api_base::realtime::origin_read_tests::__api_run_browser_login_checks_its_actual_session_deadline_after_identity_wait().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn key_login_checks_its_actual_key_deadline_after_identity_wait()
        -> ::orch_api_base::realtime::origin_read_tests::Proof {
            orch_api_base::realtime::origin_read_tests::__api_run_key_login_checks_its_actual_key_deadline_after_identity_wait().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn real_socket_waits_capacity_before_detaching_a_projected_frame()
        -> ::orch_api_base::realtime::origin_read_tests::Proof {
            orch_api_base::realtime::origin_read_tests::__api_run_real_socket_waits_capacity_before_detaching_a_projected_frame().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn raw_session_expiry_cancels_an_unaccepted_real_socket_frame()
        -> ::orch_api_base::realtime::origin_read_tests::Proof {
            orch_api_base::realtime::origin_read_tests::__api_run_raw_session_expiry_cancels_an_unaccepted_real_socket_frame().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn reauthentication_after_subscription_check_retires_the_old_socket_projection()
        -> ::orch_api_base::realtime::origin_read_tests::Proof {
            orch_api_base::realtime::origin_read_tests::__api_run_reauthentication_after_subscription_check_retires_the_old_socket_projection().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_rejected_resource_frame_keeps_its_same_row_replacement_available()
        -> ::orch_api_base::realtime::origin_read_tests::Proof {
            orch_api_base::realtime::origin_read_tests::__api_run_a_rejected_resource_frame_keeps_its_same_row_replacement_available().await
        }
        #[cfg(test)]
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_connection_ends_though_its_writer_waits_inside_the_lock_of_a_subscription()
        -> ::orch_api_base::realtime::origin_read_tests::Proof {
            orch_api_base::realtime::origin_read_tests::__api_run_a_connection_ends_though_its_writer_waits_inside_the_lock_of_a_subscription().await
        }
    }

    pub mod presence {

        pub use orch_api_base::realtime::presence::REFRESH_SECS;

        pub use orch_api_base::realtime::presence::BELIEVED_SECS;

        pub use orch_api_base::realtime::presence::Verdict;

        pub use orch_api_base::realtime::presence::Presence;
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn a_person_is_listed_from_their_first_connection_until_their_last_one_leaves() {
                orch_api_base::realtime::presence::tests::__api_run_a_person_is_listed_from_their_first_connection_until_their_last_one_leaves()
            }
            #[cfg(test)]
            #[test]
            fn a_connection_that_ends_while_showing_a_feed_takes_its_reader_with_it() {
                orch_api_base::realtime::presence::tests::__api_run_a_connection_that_ends_while_showing_a_feed_takes_its_reader_with_it()
            }
            #[cfg(test)]
            #[test]
            fn what_other_processes_say_is_listed_with_what_this_one_knows_and_the_longest_reader_comes_first()
             {
                orch_api_base::realtime::presence::tests::__api_run_what_other_processes_say_is_listed_with_what_this_one_knows_and_the_longest_reader_comes_first()
            }
            #[cfg(test)]
            #[test]
            fn a_process_that_stops_saying_it_is_there_is_forgotten_after_three_refreshes() {
                orch_api_base::realtime::presence::tests::__api_run_a_process_that_stops_saying_it_is_there_is_forgotten_after_three_refreshes()
            }
            #[cfg(test)]
            #[test]
            fn what_this_process_knows_for_the_others_is_only_what_its_own_connections_show() {
                orch_api_base::realtime::presence::tests::__api_run_what_this_process_knows_for_the_others_is_only_what_its_own_connections_show()
            }
            #[cfg(test)]
            #[test]
            fn a_list_is_sent_at_once_the_first_time_and_then_at_most_once_a_second() {
                orch_api_base::realtime::presence::tests::__api_run_a_list_is_sent_at_once_the_first_time_and_then_at_most_once_a_second()
            }
        }
    }
    #[cfg(test)]
    mod resource_health {
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn unchanged_measures_send_no_new_status_just_because_time_passed() {
                orch_api_base::realtime::resource_health::tests::__api_run_unchanged_measures_send_no_new_status_just_because_time_passed()
            }
        }
    }

    pub mod rfb_filter {

        pub use orch_api_base::realtime::rfb_filter::MAX_INPUT;

        pub use orch_api_base::realtime::rfb_filter::ProtocolError;

        pub use orch_api_base::realtime::rfb_filter::ClientMessage;

        pub use orch_api_base::realtime::rfb_filter::Filter;
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn keys_clicks_paste_resize_and_power_leave_the_next_picture_request_intact() {
                orch_api_base::realtime::rfb_filter::tests::__api_run_keys_clicks_paste_resize_and_power_leave_the_next_picture_request_intact()
            }
            #[cfg(test)]
            #[test]
            fn picture_setup_continuous_updates_and_fences_pass_on_a_watching_stream() {
                orch_api_base::realtime::rfb_filter::tests::__api_run_picture_setup_continuous_updates_and_fences_pass_on_a_watching_stream()
            }
            #[cfg(test)]
            #[test]
            fn unknown_and_oversized_messages_end_before_any_following_action_can_be_misread() {
                orch_api_base::realtime::rfb_filter::tests::__api_run_unknown_and_oversized_messages_end_before_any_following_action_can_be_misread()
            }
        }
    }

    pub mod terminals {

        pub use orch_api_base::realtime::terminals::Door;

        pub use orch_api_base::realtime::terminals::Ticket;

        pub use orch_api_base::realtime::terminals::Told;

        pub use orch_api_base::realtime::terminals::Sockets;

        pub use orch_api_base::realtime::terminals::shell_socket;

        pub use orch_api_base::realtime::terminals::agent_socket;
        #[cfg(test)]
        mod surface_tests {
            #[cfg(test)]
            #[test]
            fn surface_terminal_ticket_keeps_its_original_door_until_matching_redemption() {
                orch_api_base::realtime::terminals::surface_tests::__api_run_surface_terminal_ticket_keeps_its_original_door_until_matching_redemption()
            }
        }
    }
    #[cfg(test)]
    mod ticket_scope_tests {
        #[cfg(test)]
        mod terminal_write_socket_tests {
            #[cfg(test)]
            #[tokio::test(flavor = "multi_thread")]
            async fn actual_full_terminal_pump_resumes_queued_marker_with_unchanged_authority()
            -> ::orch_api_base::realtime::ticket_scope_tests::terminal_write_socket_tests::Proof
            {
                orch_api_base::realtime::ticket_scope_tests::terminal_write_socket_tests::__api_run_actual_full_terminal_pump_resumes_queued_marker_with_unchanged_authority().await
            }
            #[cfg(test)]
            #[tokio::test(flavor = "multi_thread")]
            async fn actual_full_terminal_pump_stops_queued_marker_after_same_key_withdrawal()
            -> ::orch_api_base::realtime::ticket_scope_tests::terminal_write_socket_tests::Proof
            {
                orch_api_base::realtime::ticket_scope_tests::terminal_write_socket_tests::__api_run_actual_full_terminal_pump_stops_queued_marker_after_same_key_withdrawal().await
            }
        }
        #[cfg(test)]
        mod session_ticket_wire_tests {
            #[cfg(test)]
            #[tokio::test(flavor = "multi_thread")]
            async fn actual_browser_control_keeps_own_read_but_stops_native_input_and_driving_after_manage_loss()
            -> ::orch_api_base::session_ticket_support::Proof {
                orch_api_base::realtime::ticket_scope_tests::session_ticket_wire_tests::__api_run_actual_browser_control_keeps_own_read_but_stops_native_input_and_driving_after_manage_loss().await
            }
            #[cfg(test)]
            #[tokio::test(flavor = "multi_thread")]
            async fn actual_display_control_keeps_own_read_but_stops_native_key_and_driving_after_manage_loss()
            -> ::orch_api_base::session_ticket_support::Proof {
                orch_api_base::realtime::ticket_scope_tests::session_ticket_wire_tests::__api_run_actual_display_control_keeps_own_read_but_stops_native_key_and_driving_after_manage_loss().await
            }
            #[cfg(test)]
            #[tokio::test(flavor = "multi_thread")]
            async fn actual_browser_current_read_loss_closes_before_new_native_tab_content()
            -> ::orch_api_base::session_ticket_support::Proof {
                orch_api_base::realtime::ticket_scope_tests::session_ticket_wire_tests::__api_run_actual_browser_current_read_loss_closes_before_new_native_tab_content().await
            }
            #[cfg(test)]
            #[tokio::test(flavor = "multi_thread")]
            async fn actual_display_current_read_loss_closes_before_new_native_pixel_bytes()
            -> ::orch_api_base::session_ticket_support::Proof {
                orch_api_base::realtime::ticket_scope_tests::session_ticket_wire_tests::__api_run_actual_display_current_read_loss_closes_before_new_native_pixel_bytes().await
            }
            #[cfg(test)]
            #[tokio::test(flavor = "multi_thread")]
            async fn actual_revoked_browser_ticket_never_runs_native_ensure_before_its_current_gate()
            -> ::orch_api_base::session_ticket_support::Proof {
                orch_api_base::realtime::ticket_scope_tests::session_ticket_wire_tests::__api_run_actual_revoked_browser_ticket_never_runs_native_ensure_before_its_current_gate().await
            }
            #[cfg(test)]
            #[tokio::test(flavor = "multi_thread")]
            async fn actual_display_rechecks_revoked_ticket_after_native_version_wait_before_first_bytes()
            -> ::orch_api_base::session_ticket_support::Proof {
                orch_api_base::realtime::ticket_scope_tests::session_ticket_wire_tests::__api_run_actual_display_rechecks_revoked_ticket_after_native_version_wait_before_first_bytes().await
            }
            #[cfg(test)]
            #[tokio::test(flavor = "multi_thread")]
            async fn an_actual_started_rfb_message_completes_only_its_own_tail_after_manage_loss()
            -> ::orch_api_base::session_ticket_support::Proof {
                orch_api_base::realtime::ticket_scope_tests::session_ticket_wire_tests::__api_run_an_actual_started_rfb_message_completes_only_its_own_tail_after_manage_loss().await
            }
            #[cfg(test)]
            #[tokio::test(flavor = "multi_thread")]
            async fn an_actual_zero_byte_queued_rfb_message_rechecks_manage_after_real_pending()
            -> ::orch_api_base::session_ticket_support::Proof {
                orch_api_base::realtime::ticket_scope_tests::session_ticket_wire_tests::__api_run_an_actual_zero_byte_queued_rfb_message_rechecks_manage_after_real_pending().await
            }
        }
        #[cfg(test)]
        #[tokio::test]
        async fn browser_view_ticket_redeem_rechecks_current_own_read()
        -> ::orch_api_base::session_ticket_support::Proof {
            orch_api_base::realtime::ticket_scope_tests::__api_run_browser_view_ticket_redeem_rechecks_current_own_read().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn browser_control_ticket_keeps_own_read_but_drops_current_manage_after_narrowing()
        -> ::orch_api_base::session_ticket_support::Proof {
            orch_api_base::realtime::ticket_scope_tests::__api_run_browser_control_ticket_keeps_own_read_but_drops_current_manage_after_narrowing().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn display_view_ticket_redeem_rechecks_current_own_read()
        -> ::orch_api_base::session_ticket_support::Proof {
            orch_api_base::realtime::ticket_scope_tests::__api_run_display_view_ticket_redeem_rechecks_current_own_read().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn display_control_ticket_keeps_own_read_but_drops_current_manage_after_narrowing()
        -> ::orch_api_base::session_ticket_support::Proof {
            orch_api_base::realtime::ticket_scope_tests::__api_run_display_control_ticket_keeps_own_read_but_drops_current_manage_after_narrowing().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn browser_view_only_ticket_never_escalates_into_input_after_scope_change()
        -> ::orch_api_base::session_ticket_support::Proof {
            orch_api_base::realtime::ticket_scope_tests::__api_run_browser_view_only_ticket_never_escalates_into_input_after_scope_change().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn display_view_only_ticket_never_escalates_into_input_after_scope_change()
        -> ::orch_api_base::session_ticket_support::Proof {
            orch_api_base::realtime::ticket_scope_tests::__api_run_display_view_only_ticket_never_escalates_into_input_after_scope_change().await
        }
    }

    pub mod user {

        pub use orch_api_base::realtime::user::open_path;

        pub use orch_api_base::realtime::user::notification_view;

        pub use orch_api_base::realtime::user::notification_files;
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn stored_file_metadata_is_removed_from_both_the_notice_and_its_action_copy() {
                orch_api_base::realtime::user::tests::__api_run_stored_file_metadata_is_removed_from_both_the_notice_and_its_action_copy()
            }
            #[cfg(test)]
            #[test]
            fn a_notification_opens_the_path_its_kind_named_else_its_thread_else_the_approvals_else_its_place_else_the_inbox()
             {
                orch_api_base::realtime::user::tests::__api_run_a_notification_opens_the_path_its_kind_named_else_its_thread_else_the_approvals_else_its_place_else_the_inbox()
            }
            #[cfg(test)]
            #[test]
            fn a_path_that_leaves_the_application_is_not_followed() {
                orch_api_base::realtime::user::tests::__api_run_a_path_that_leaves_the_application_is_not_followed()
            }
            #[cfg(test)]
            #[test]
            fn a_row_of_a_kind_nobody_knows_is_no_notification() {
                orch_api_base::realtime::user::tests::__api_run_a_row_of_a_kind_nobody_knows_is_no_notification()
            }
            #[cfg(test)]
            #[test]
            fn the_place_a_row_names_is_shown_as_the_reader_may_know_it() {
                orch_api_base::realtime::user::tests::__api_run_the_place_a_row_names_is_shown_as_the_reader_may_know_it()
            }
        }
    }

    pub mod watches {
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn the_processes_and_the_load_need_terminal_view_and_a_directory_files_view_on_the_environment()
             {
                orch_api_base::realtime::watches::tests::__api_run_the_processes_and_the_load_need_terminal_view_and_a_directory_files_view_on_the_environment()
            }
            #[cfg(test)]
            #[test]
            fn no_other_feed_is_an_environments_to_decide_by_its_rows() {
                orch_api_base::realtime::watches::tests::__api_run_no_other_feed_is_an_environments_to_decide_by_its_rows()
            }
        }
    }
    #[cfg(test)]
    mod tests {
        #[cfg(test)]
        #[test]
        fn every_feed_with_a_sequence_has_a_source_and_the_users_own_feed_is_theirs_alone() {
            orch_api_base::realtime::tests::__api_run_every_feed_with_a_sequence_has_a_source_and_the_users_own_feed_is_theirs_alone()
        }
        #[cfg(test)]
        #[test]
        fn a_feed_without_a_sequence_or_without_data_here_has_no_source() {
            orch_api_base::realtime::tests::__api_run_a_feed_without_a_sequence_or_without_data_here_has_no_source()
        }
        #[cfg(test)]
        #[test]
        fn the_times_a_deployment_does_not_set_are_the_protocols_and_the_two_buffer_keys_are_the_deployments()
         {
            orch_api_base::realtime::tests::__api_run_the_times_a_deployment_does_not_set_are_the_protocols_and_the_two_buffer_keys_are_the_deployments()
        }
    }

    pub mod topics {}

    pub mod chats {}

    pub mod raw_traffic {

        pub use orch_api_base::realtime::raw_traffic::Boundary;

        pub use orch_api_base::realtime::raw_traffic::before;

        pub use orch_api_base::realtime::raw_traffic::after;
    }

    pub mod tree {}

    pub mod environments {}

    pub mod subscriptions {

        pub use orch_api_base::realtime::subscriptions::Subscription;
    }

    pub mod documents {}

    pub mod usage {}

    pub mod sync {}

    pub mod background_watches {}

    pub mod resource_changes {}

    pub mod threads {}

    pub mod resources {}

    pub mod server {

        pub use orch_api_base::realtime::server::Connection;

        pub use orch_api_base::realtime::server::socket;
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[tokio::test]
            async fn a_writer_left_behind_by_a_reader_that_ended_holds_nothing_afterwards() {
                orch_api_base::realtime::server::tests::__api_run_a_writer_left_behind_by_a_reader_that_ended_holds_nothing_afterwards().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn a_reader_left_behind_by_a_writer_that_ended_holds_nothing_afterwards() {
                orch_api_base::realtime::server::tests::__api_run_a_reader_left_behind_by_a_writer_that_ended_holds_nothing_afterwards().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn a_half_that_is_only_left_behind_keeps_its_lock() {
                orch_api_base::realtime::server::tests::__api_run_a_half_that_is_only_left_behind_keeps_its_lock().await
            }
        }
    }

    pub mod environment_watches {}
}

pub mod registry {

    pub use orch_api_base::registry::OperationFuture;

    pub use orch_api_base::registry::OperationResolver;

    pub use orch_api_base::registry::OptionalOperationFuture;

    pub use orch_api_base::registry::OptionalOperationResolver;

    pub use orch_api_base::registry::OperationInitialFuture;

    pub use orch_api_base::registry::OperationInitial;

    pub use orch_api_base::registry::Ctx;

    pub use orch_api_base::registry::Answer;

    pub use orch_api_base::registry::Registry;

    pub use orch_api_base::registry::CreationCoverage;

    pub use orch_api_base::registry::OperationPlan;
    #[cfg(test)]
    mod action_guard_state_tests {
        #[cfg(test)]
        #[tokio::test]
        async fn provisional_disclosure_and_operands_cannot_replace_final_or_opened_state() {
            orch_api_base::registry::action_guard_state_tests::__api_run_provisional_disclosure_and_operands_cannot_replace_final_or_opened_state().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn current_user_checkpoint_cannot_replace_original_authentication_ceiling() {
            orch_api_base::registry::action_guard_state_tests::__api_run_current_user_checkpoint_cannot_replace_original_authentication_ceiling().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn actual_http_identity_intersects_effective_original_and_current_admin_standing() {
            orch_api_base::registry::action_guard_state_tests::__api_run_actual_http_identity_intersects_effective_original_and_current_admin_standing().await
        }
    }
    #[cfg(test)]
    mod query_tests {
        #[cfg(test)]
        #[test]
        fn storage_filters_and_flattened_paging_decode_through_the_real_query_adapter()
        -> ::orch_api_base::middleware::requests::ApiResult<()> {
            orch_api_base::registry::query_tests::__api_run_storage_filters_and_flattened_paging_decode_through_the_real_query_adapter()
        }
    }
}
#[cfg(test)]
pub(crate) mod response_body {
    #[cfg(test)]
    #[allow(clippy::unwrap_used, reason = "transport controls fail by panicking")]
    mod tests {
        #[cfg(test)]
        #[tokio::test]
        async fn original_parts_data_trailers_and_upstream_errors_are_preserved() {
            orch_api_base::response_body::tests::__api_run_original_parts_data_trailers_and_upstream_errors_are_preserved().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn current_check_occurs_after_an_awaited_upstream_frame() {
            orch_api_base::response_body::tests::__api_run_current_check_occurs_after_an_awaited_upstream_frame().await
        }
        #[cfg(test)]
        #[tokio::test]
        async fn metadata_only_replies_are_checked_before_returning_their_headers() {
            orch_api_base::response_body::tests::__api_run_metadata_only_replies_are_checked_before_returning_their_headers().await
        }
    }
}

pub mod sign_in {

    pub use orch_api_base::sign_in::NOT_KEPT;

    pub use orch_api_base::sign_in::SEND_GAP;

    pub use orch_api_base::sign_in::SIGNUP_LIMIT;

    pub use orch_api_base::sign_in::SIGNUP_WINDOW;

    pub use orch_api_base::sign_in::KNOWN_BROWSERS_PER_PERSON;

    pub use orch_api_base::sign_in::Limits;

    pub use orch_api_base::sign_in::SystemPolicy;

    pub use orch_api_base::sign_in::setting;

    pub use orch_api_base::sign_in::system_policy;

    pub use orch_api_base::sign_in::origin;

    pub use orch_api_base::sign_in::user_agent;

    pub use orch_api_base::sign_in::record;

    pub use orch_api_base::sign_in::cookie;

    pub use orch_api_base::sign_in::session_cookie;

    pub use orch_api_base::sign_in::known_for;

    pub use orch_api_base::sign_in::device_cookie;

    pub use orch_api_base::sign_in::cleared_session_cookie;

    pub use orch_api_base::sign_in::session_life;

    pub use orch_api_base::sign_in::access_life;

    pub use orch_api_base::sign_in::Opened;

    pub use orch_api_base::sign_in::remember_browser;

    pub use orch_api_base::sign_in::open_session;

    pub mod accounts {

        pub use orch_api_base::sign_in::accounts::Details;

        pub use orch_api_base::sign_in::accounts::Clean;

        pub use orch_api_base::sign_in::accounts::email_fits;

        pub use orch_api_base::sign_in::accounts::check;

        pub use orch_api_base::sign_in::accounts::locale_of;

        pub use orch_api_base::sign_in::accounts::How;

        pub use orch_api_base::sign_in::accounts::taken;

        pub use orch_api_base::sign_in::accounts::make;
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn a_username_is_lower_case_letters_digits_hyphens_and_underscores() {
                orch_api_base::sign_in::accounts::tests::__api_run_a_username_is_lower_case_letters_digits_hyphens_and_underscores()
            }
            #[cfg(test)]
            #[test]
            fn what_a_person_gives_is_checked_all_at_once_and_tidied() {
                orch_api_base::sign_in::accounts::tests::__api_run_what_a_person_gives_is_checked_all_at_once_and_tidied()
            }
            #[cfg(test)]
            #[test]
            fn a_new_person_starts_in_the_language_their_browser_asks_for_among_the_two_shipped() {
                orch_api_base::sign_in::accounts::tests::__api_run_a_new_person_starts_in_the_language_their_browser_asks_for_among_the_two_shipped()
            }
            #[cfg(test)]
            #[test]
            fn an_address_has_something_before_the_at_and_a_dotted_name_after_it() {
                orch_api_base::sign_in::accounts::tests::__api_run_an_address_has_something_before_the_at_and_a_dotted_name_after_it()
            }
        }
    }

    pub mod codes {

        pub use orch_api_base::sign_in::codes::ALPHABET;

        pub use orch_api_base::sign_in::codes::CODE_CHARS;

        pub use orch_api_base::sign_in::codes::new_code;

        pub use orch_api_base::sign_in::codes::display;

        pub use orch_api_base::sign_in::codes::normalize;

        pub use orch_api_base::sign_in::codes::hash;

        pub use orch_api_base::sign_in::codes::matches;

        pub use orch_api_base::sign_in::codes::TOTP_DIGITS;

        pub use orch_api_base::sign_in::codes::TOTP_STEP_SECS;

        pub use orch_api_base::sign_in::codes::TOTP_SKEW;

        pub use orch_api_base::sign_in::codes::TOTP_SECRET_BYTES;

        pub use orch_api_base::sign_in::codes::AuthenticatorError;

        pub use orch_api_base::sign_in::codes::new_secret;

        pub use orch_api_base::sign_in::codes::authenticator;

        pub use orch_api_base::sign_in::codes::totp_step;

        pub use orch_api_base::sign_in::codes::MASTER_KEY_PATH;

        pub use orch_api_base::sign_in::codes::MASTER_KEY_VARIABLE;

        pub use orch_api_base::sign_in::codes::install_master_key;

        pub use orch_api_base::sign_in::codes::seal_secret;

        pub use orch_api_base::sign_in::codes::open_secret;
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn a_mailed_code_is_eight_characters_that_cannot_be_mistaken_for_one_another() {
                orch_api_base::sign_in::codes::tests::__api_run_a_mailed_code_is_eight_characters_that_cannot_be_mistaken_for_one_another()
            }
            #[cfg(test)]
            #[test]
            fn a_code_is_read_without_regard_to_case_hyphen_or_space() {
                orch_api_base::sign_in::codes::tests::__api_run_a_code_is_read_without_regard_to_case_hyphen_or_space()
            }
            #[cfg(test)]
            #[test]
            fn only_the_hash_of_a_code_for_one_person_matches_it() {
                orch_api_base::sign_in::codes::tests::__api_run_only_the_hash_of_a_code_for_one_person_matches_it()
            }
            #[cfg(test)]
            #[test]
            fn a_code_of_an_authenticator_is_six_digits_of_the_step_it_is_shown_in() {
                orch_api_base::sign_in::codes::tests::__api_run_a_code_of_an_authenticator_is_six_digits_of_the_step_it_is_shown_in()
            }
            #[cfg(test)]
            #[test]
            fn a_colon_in_a_label_does_not_end_it() {
                orch_api_base::sign_in::codes::tests::__api_run_a_colon_in_a_label_does_not_end_it()
            }
            #[cfg(test)]
            #[test]
            fn a_sealed_secret_opens_for_its_person_alone() {
                orch_api_base::sign_in::codes::tests::__api_run_a_sealed_secret_opens_for_its_person_alone()
            }
        }
    }

    pub mod mail {

        pub use orch_api_base::sign_in::mail::CHANNEL_KIND;

        pub use orch_api_base::sign_in::mail::VERIFY_LINK_LIFE;

        pub use orch_api_base::sign_in::mail::RESET_LINK_LIFE;

        pub use orch_api_base::sign_in::mail::Addressee;

        pub use orch_api_base::sign_in::mail::Message;

        pub use orch_api_base::sign_in::mail::link;

        pub use orch_api_base::sign_in::mail::payload;

        pub use orch_api_base::sign_in::mail::EMAIL_TOKEN_PREFIX;

        pub use orch_api_base::sign_in::mail::Purpose;

        pub use orch_api_base::sign_in::mail::Link;

        pub use orch_api_base::sign_in::mail::send_link;

        pub use orch_api_base::sign_in::mail::send_code;

        pub use orch_api_base::sign_in::mail::send_notice;
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn a_link_is_the_public_origin_the_page_and_the_secret() {
                orch_api_base::sign_in::mail::tests::__api_run_a_link_is_the_public_origin_the_page_and_the_secret()
            }
            #[cfg(test)]
            #[test]
            fn a_message_names_its_purpose_its_addressee_and_what_the_worker_needs() {
                orch_api_base::sign_in::mail::tests::__api_run_a_message_names_its_purpose_its_addressee_and_what_the_worker_needs()
            }
        }
    }

    pub mod passwords {

        pub use orch_api_base::sign_in::passwords::MEMORY_KIB;

        pub use orch_api_base::sign_in::passwords::PASSES;

        pub use orch_api_base::sign_in::passwords::LANES;

        pub use orch_api_base::sign_in::passwords::HashFailed;

        pub use orch_api_base::sign_in::passwords::hash;

        pub use orch_api_base::sign_in::passwords::verify;

        pub use orch_api_base::sign_in::passwords::Refused;

        pub use orch_api_base::sign_in::passwords::COMMON_SENTENCE;

        pub use orch_api_base::sign_in::passwords::check_new;
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[tokio::test]
            async fn a_password_is_kept_as_an_argon2id_hash_that_only_it_opens() {
                orch_api_base::sign_in::passwords::tests::__api_run_a_password_is_kept_as_an_argon2id_hash_that_only_it_opens().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn no_hash_and_a_hash_that_does_not_read_are_never_right() {
                orch_api_base::sign_in::passwords::tests::__api_run_no_hash_and_a_hash_that_does_not_read_are_never_right().await
            }
            #[cfg(test)]
            #[tokio::test]
            async fn a_name_nobody_has_is_checked_with_the_work_of_one_somebody_has() {
                orch_api_base::sign_in::passwords::tests::__api_run_a_name_nobody_has_is_checked_with_the_work_of_one_somebody_has().await
            }
            #[cfg(test)]
            #[test]
            fn a_new_password_has_enough_characters_and_is_not_the_name() {
                orch_api_base::sign_in::passwords::tests::__api_run_a_new_password_has_enough_characters_and_is_not_the_name()
            }
            #[cfg(test)]
            #[test]
            fn the_release_carries_the_pinned_list_of_common_passwords_and_the_value_it_adds() {
                orch_api_base::sign_in::passwords::tests::__api_run_the_release_carries_the_pinned_list_of_common_passwords_and_the_value_it_adds()
            }
            #[cfg(test)]
            #[test]
            fn a_common_password_is_refused_whole_and_case_folded_and_a_phrase_that_holds_one_is_not()
             {
                orch_api_base::sign_in::passwords::tests::__api_run_a_common_password_is_refused_whole_and_case_folded_and_a_phrase_that_holds_one_is_not()
            }
        }
    }

    pub mod people {

        pub use orch_api_base::sign_in::people::avatar_text;

        pub use orch_api_base::sign_in::people::user_ref;

        pub use orch_api_base::sign_in::people::email_code_uses;

        pub use orch_api_base::sign_in::people::me;
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn an_avatar_shows_the_first_letter_as_a_person_reads_it_in_its_upper_case() {
                orch_api_base::sign_in::people::tests::__api_run_an_avatar_shows_the_first_letter_as_a_person_reads_it_in_its_upper_case()
            }
        }
    }

    pub mod proofs {

        pub use orch_api_base::sign_in::proofs::Capabilities;

        pub use orch_api_base::sign_in::proofs::capabilities;

        pub use orch_api_base::sign_in::proofs::capabilities_with_recovery;

        pub use orch_api_base::sign_in::proofs::password_right;

        pub use orch_api_base::sign_in::proofs::mailed_code_right;

        pub use orch_api_base::sign_in::proofs::authenticator_right;

        pub use orch_api_base::sign_in::proofs::recovery_right;
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn a_person_with_no_second_step_is_in_with_a_password_or_a_mailed_code_where_it_is_allowed()
             {
                orch_api_base::sign_in::proofs::tests::__api_run_a_person_with_no_second_step_is_in_with_a_password_or_a_mailed_code_where_it_is_allowed()
            }
            #[cfg(test)]
            #[test]
            fn a_person_with_an_authenticator_needs_it_beside_the_password_or_a_mailed_code() {
                orch_api_base::sign_in::proofs::tests::__api_run_a_person_with_an_authenticator_needs_it_beside_the_password_or_a_mailed_code()
            }
            #[cfg(test)]
            #[test]
            fn a_mailed_code_as_the_only_second_step_asks_for_the_password_after_a_mailed_code() {
                orch_api_base::sign_in::proofs::tests::__api_run_a_mailed_code_as_the_only_second_step_asks_for_the_password_after_a_mailed_code()
            }
            #[cfg(test)]
            #[test]
            fn an_unused_recovery_code_replaces_the_app_but_never_the_first_factor() {
                orch_api_base::sign_in::proofs::tests::__api_run_an_unused_recovery_code_replaces_the_app_but_never_the_first_factor()
            }
            #[cfg(test)]
            #[test]
            fn a_person_with_both_second_steps_may_finish_with_either_in_the_order_of_the_contract()
            {
                orch_api_base::sign_in::proofs::tests::__api_run_a_person_with_both_second_steps_may_finish_with_either_in_the_order_of_the_contract()
            }
        }
    }

    pub mod recovery_codes {

        pub use orch_api_base::sign_in::recovery_codes::CODE_COUNT;

        pub use orch_api_base::sign_in::recovery_codes::CODE_CHARS;

        pub use orch_api_base::sign_in::recovery_codes::issue;

        pub use orch_api_base::sign_in::recovery_codes::normalize;

        pub use orch_api_base::sign_in::recovery_codes::hash;

        pub use orch_api_base::sign_in::recovery_codes::hashes;

        pub use orch_api_base::sign_in::recovery_codes::factor_hash;

        pub use orch_api_base::sign_in::recovery_codes::grant_management;
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn issued_codes_can_be_saved_and_typed_with_spaces_or_lower_case() {
                orch_api_base::sign_in::recovery_codes::tests::__api_run_issued_codes_can_be_saved_and_typed_with_spaces_or_lower_case()
            }
            #[cfg(test)]
            #[test]
            fn a_saved_code_is_bound_to_its_person_and_is_not_a_mailed_code_hash() {
                orch_api_base::sign_in::recovery_codes::tests::__api_run_a_saved_code_is_bound_to_its_person_and_is_not_a_mailed_code_hash()
            }
        }
    }

    pub mod second_step {

        pub use orch_api_base::sign_in::second_step::SECURITY_PATH;

        pub use orch_api_base::sign_in::second_step::LAST_SEEN_EVERY;

        pub use orch_api_base::sign_in::second_step::methods;

        pub use orch_api_base::sign_in::second_step::requirement;

        pub use orch_api_base::sign_in::second_step::places;

        pub use orch_api_base::sign_in::second_step::hold;

        pub use orch_api_base::sign_in::second_step::refusal;

        pub use orch_api_base::sign_in::second_step::of_members;

        pub use orch_api_base::sign_in::second_step::of_me;

        pub use orch_api_base::sign_in::second_step::Requirement;

        pub use orch_api_base::sign_in::second_step::configured;
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn a_requirement_counts_where_it_is_set_and_not_only_what_waits_for_it() {
                orch_api_base::sign_in::second_step::tests::__api_run_a_requirement_counts_where_it_is_set_and_not_only_what_waits_for_it()
            }
            #[cfg(test)]
            #[test]
            fn last_seen_is_written_at_most_once_a_heartbeat() {
                orch_api_base::sign_in::second_step::tests::__api_run_last_seen_is_written_at_most_once_a_heartbeat()
            }
        }
    }
    #[cfg(test)]
    mod tests {
        #[cfg(test)]
        #[test]
        fn the_limits_as_shipped_are_eight_a_name_in_three_hours_thirty_an_address_in_a_quarter_hour_and_five_codes_in_three_hours()
         {
            orch_api_base::sign_in::tests::__api_run_the_limits_as_shipped_are_eight_a_name_in_three_hours_thirty_an_address_in_a_quarter_hour_and_five_codes_in_three_hours()
        }
        #[cfg(test)]
        #[test]
        fn a_cookie_carries_the_attributes_every_cookie_of_the_sign_in_has() {
            orch_api_base::sign_in::tests::__api_run_a_cookie_carries_the_attributes_every_cookie_of_the_sign_in_has()
        }
    }

    pub mod throttle {

        pub use orch_api_base::sign_in::throttle::DEVICE_MARK_PREFIX;

        pub use orch_api_base::sign_in::throttle::Who;

        pub use orch_api_base::sign_in::throttle::normalize;

        pub use orch_api_base::sign_in::throttle::name_hash;

        pub use orch_api_base::sign_in::throttle::mark_hash;

        pub use orch_api_base::sign_in::throttle::mark_of;

        pub use orch_api_base::sign_in::throttle::Located;

        pub use orch_api_base::sign_in::throttle::locate;

        pub use orch_api_base::sign_in::throttle::Tried;

        pub use orch_api_base::sign_in::throttle::rate_limited;

        pub use orch_api_base::sign_in::throttle::begin;

        pub use orch_api_base::sign_in::throttle::end;

        pub use orch_api_base::sign_in::throttle::by_address;

        pub use orch_api_base::sign_in::throttle::identity_start;

        pub use orch_api_base::sign_in::throttle::identity_account;
    }

    pub mod fresh {

        pub use orch_api_base::sign_in::fresh::Purpose;

        pub use orch_api_base::sign_in::fresh::Proven;

        pub use orch_api_base::sign_in::fresh::prove;
    }

    pub mod reset_links {

        pub use orch_api_base::sign_in::reset_links::Made;

        pub use orch_api_base::sign_in::reset_links::make;
    }

    pub mod login {

        pub use orch_api_base::sign_in::login::invalid_credentials;

        pub use orch_api_base::sign_in::login::second_step_invalid;

        pub use orch_api_base::sign_in::login::second_step_required;

        pub use orch_api_base::sign_in::login::login;

        pub use orch_api_base::sign_in::login::send_code;
    }
}

pub mod syncs {

    pub use orch_api_base::syncs::Reading;

    pub use orch_api_base::syncs::Piece;

    pub use orch_api_base::syncs::Member;

    pub use orch_api_base::syncs::begin;

    pub use orch_api_base::syncs::put_together;
    #[cfg(test)]
    mod tests {
        #[cfg(test)]
        #[test]
        fn the_reply_is_made_of_the_members_and_the_numbers_it_ends_with() {
            orch_api_base::syncs::tests::__api_run_the_reply_is_made_of_the_members_and_the_numbers_it_ends_with()
        }
        #[cfg(test)]
        #[test]
        fn one_member_that_found_too_much_makes_the_whole_incomplete() {
            orch_api_base::syncs::tests::__api_run_one_member_that_found_too_much_makes_the_whole_incomplete()
        }
        #[cfg(test)]
        #[test]
        fn two_members_of_one_name_are_a_defect_and_not_an_answer() {
            orch_api_base::syncs::tests::__api_run_two_members_of_one_name_are_a_defect_and_not_an_answer()
        }
    }
}

pub mod thread_engine {

    pub use orch_api_base::thread_engine::DispatchCommands;

    pub use orch_api_base::thread_engine::EngineApplier;

    pub use orch_api_base::thread_engine::detached;

    pub use orch_api_base::thread_engine::detached_with;

    pub use orch_api_base::thread_engine::start;

    pub use orch_api_base::thread_engine::start_with;

    pub use orch_api_base::thread_engine::start_with_blobs;
    #[cfg(test)]
    mod tests {
        #[cfg(test)]
        #[test]
        fn the_words_of_a_commands_row_are_the_stages_the_engine_knows() {
            orch_api_base::thread_engine::tests::__api_run_the_words_of_a_commands_row_are_the_stages_the_engine_knows()
        }
        #[cfg(test)]
        #[test]
        fn a_failed_database_is_reported_as_the_databases_and_anything_else_sends_the_events_again()
        {
            orch_api_base::thread_engine::tests::__api_run_a_failed_database_is_reported_as_the_databases_and_anything_else_sends_the_events_again()
        }
        #[cfg(test)]
        #[test]
        fn what_the_dispatcher_cannot_do_is_told_in_the_engines_words() {
            orch_api_base::thread_engine::tests::__api_run_what_the_dispatcher_cannot_do_is_told_in_the_engines_words()
        }
    }
}

pub mod tickets {

    pub use orch_api_base::tickets::DEFAULT_TICKET_SECS;

    pub use orch_api_base::tickets::TERMINAL_TICKET_SECS;

    pub use orch_api_base::tickets::TicketFault;

    pub use orch_api_base::tickets::Tickets;

    pub use orch_api_base::tickets::request;
    #[cfg(test)]
    mod tests {
        #[cfg(test)]
        #[test]
        fn a_host_publication_verifies_as_its_named_node_and_keeps_the_opening_separate_from_its_ticket()
         {
            orch_api_base::tickets::tests::__api_run_a_host_publication_verifies_as_its_named_node_and_keeps_the_opening_separate_from_its_ticket()
        }
        #[cfg(test)]
        #[test]
        fn a_request_in_a_stored_command_becomes_the_ticket_of_the_machine_it_goes_to() {
            orch_api_base::tickets::tests::__api_run_a_request_in_a_stored_command_becomes_the_ticket_of_the_machine_it_goes_to()
        }
        #[cfg(test)]
        #[test]
        fn a_ticket_is_good_for_nobody_but_the_machine_it_was_signed_for() {
            orch_api_base::tickets::tests::__api_run_a_ticket_is_good_for_nobody_but_the_machine_it_was_signed_for()
        }
        #[cfg(test)]
        #[test]
        fn a_tool_ticket_signs_one_environment_and_refuses_an_unscoped_request() {
            orch_api_base::tickets::tests::__api_run_a_tool_ticket_signs_one_environment_and_refuses_an_unscoped_request()
        }
        #[cfg(test)]
        #[test]
        fn a_ticket_for_a_terminal_is_the_systems_own_and_short_lived() {
            orch_api_base::tickets::tests::__api_run_a_ticket_for_a_terminal_is_the_systems_own_and_short_lived()
        }
        #[cfg(test)]
        #[test]
        fn a_request_that_cannot_be_a_ticket_says_what_is_wrong_with_it() {
            orch_api_base::tickets::tests::__api_run_a_request_that_cannot_be_a_ticket_says_what_is_wrong_with_it()
        }
    }
}

pub mod routes {

    pub use orch_api_base::routes::install_master_key;

    pub mod forks {

        pub use orch_api_threads::routes::forks::mount;
        #[cfg(test)]
        mod batch_reply {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn execution_progress_keeps_reference_disclosure_but_changed_names_or_visibility_do_not()
                 {
                    orch_api_threads::routes::forks::batch_reply::tests::__api_run_execution_progress_keeps_reference_disclosure_but_changed_names_or_visibility_do_not()
                }
            }
        }
    }

    pub mod threads {

        pub use orch_api_threads::routes::threads::mount;

        pub use orch_api_threads::routes::threads::one;

        pub use orch_api_threads::routes::threads::create_failed;

        pub use orch_api_threads::routes::threads::input_failed;

        pub use orch_api_threads::routes::threads::author_only;

        pub use orch_api_threads::routes::threads::read_failed;

        pub mod continuations {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn an_admission_check_failure_retains_only_phase_and_safe_schema_identity() {
                    orch_api_threads::routes::threads::continuations::tests::__api_run_an_admission_check_failure_retains_only_phase_and_safe_schema_identity()
                }
                #[cfg(test)]
                #[test]
                fn internal_diagnostics_do_not_format_row_values_or_arbitrary_error_text() {
                    orch_api_threads::routes::threads::continuations::tests::__api_run_internal_diagnostics_do_not_format_row_values_or_arbitrary_error_text()
                }
            }
        }

        pub mod model_inputs {

            pub use orch_api_threads::routes::threads::model_inputs::PAGE_DEFAULT;

            pub use orch_api_threads::routes::threads::model_inputs::PAGE_MAX;

            pub use orch_api_threads::routes::threads::model_inputs::mount;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn a_record_is_carried_whole_with_its_words_named() {
                    orch_api_threads::routes::threads::model_inputs::tests::__api_run_a_record_is_carried_whole_with_its_words_named()
                }
                #[cfg(test)]
                #[tokio::test]
                async fn document_summary_inputs_and_their_blobs_need_the_documents_current_view_permission()
                 {
                    orch_api_threads::routes::threads::model_inputs::tests::__api_run_document_summary_inputs_and_their_blobs_need_the_documents_current_view_permission().await
                }
            }
        }

        pub mod parameters {

            pub use orch_api_threads::routes::threads::parameters::mount;
            #[cfg(test)]
            #[allow(
                clippy::unwrap_used,
                reason = "malformed parameter fixtures fail the test"
            )]
            mod tests {
                #[cfg(test)]
                #[test]
                fn capability_choices_follow_current_resolver_modes_without_rewriting_stale_choices()
                 {
                    orch_api_threads::routes::threads::parameters::tests::__api_run_capability_choices_follow_current_resolver_modes_without_rewriting_stale_choices()
                }
            }
        }

        pub mod view {

            pub use orch_api_threads::routes::threads::view::thread;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn a_harness_with_a_memory_of_its_own_keeps_the_notes_beside_the_directory_and_one_with_none_leaves_them_to_the_agent()
                 {
                    orch_api_threads::routes::threads::view::tests::__api_run_a_harness_with_a_memory_of_its_own_keeps_the_notes_beside_the_directory_and_one_with_none_leaves_them_to_the_agent()
                }
                #[cfg(test)]
                #[test]
                fn a_harness_that_runs_with_its_provider_cannot_keep_notes_in_the_directory() {
                    orch_api_threads::routes::threads::view::tests::__api_run_a_harness_that_runs_with_its_provider_cannot_keep_notes_in_the_directory()
                }
                #[cfg(test)]
                #[test]
                fn the_setting_decides_whether_anyone_keeps_notes_at_all() {
                    orch_api_threads::routes::threads::view::tests::__api_run_the_setting_decides_whether_anyone_keeps_notes_at_all()
                }
            }
        }

        pub mod export {

            pub use orch_api_threads::routes::threads::export::mount;
        }

        pub mod binding {

            pub use orch_api_threads::routes::threads::binding::mount;
        }

        pub mod native {

            pub use orch_api_threads::routes::threads::native::mount;
        }

        pub mod context {

            pub use orch_api_threads::routes::threads::context::mount;
        }

        pub mod goals {

            pub use orch_api_threads::routes::threads::goals::mount;
        }

        pub mod adopt {

            pub use orch_api_threads::routes::threads::adopt::mount;
        }

        pub mod drive {

            pub use orch_api_threads::routes::threads::drive::mount;
        }

        pub mod instructions {

            pub use orch_api_threads::routes::threads::instructions::mount;
        }

        pub mod read {

            pub use orch_api_threads::routes::threads::read::mount;
        }

        pub mod source {

            pub use orch_api_threads::routes::threads::source::mount;
        }

        pub mod background {

            pub use orch_api_threads::routes::threads::background::mount;
        }

        pub mod calls {

            pub use orch_api_threads::routes::threads::calls::mount;
        }

        pub mod raw {

            pub use orch_api_threads::routes::threads::raw::mount;
        }

        pub mod make {

            pub use orch_api_base::routes::threads::make::NEW_THREAD;

            pub use orch_api_threads::routes::threads::make::mount;
        }

        pub mod written_export {

            pub use orch_api_threads::routes::threads::written_export::markdown;
        }

        pub mod uploads {

            pub use orch_api_threads::routes::threads::uploads::mount;
        }

        pub mod rules {

            pub use orch_api_threads::routes::threads::rules::mount;
        }
    }

    pub mod documents {

        pub use orch_api_documents::routes::documents::mount;
        #[cfg(test)]
        pub(crate) mod references {
            #[cfg(test)]
            mod scopes {
                #[cfg(test)]
                #[tokio::test]
                async fn reference_cards_bind_actual_attachment_owners_and_redact_unadmitted_parent_names()
                 {
                    orch_api_documents::routes::documents::references::scopes::__api_run_reference_cards_bind_actual_attachment_owners_and_redact_unadmitted_parent_names().await
                }
                #[cfg(test)]
                #[tokio::test]
                async fn reference_cards_keep_each_actual_native_node_kind() {
                    orch_api_documents::routes::documents::references::scopes::__api_run_reference_cards_keep_each_actual_native_node_kind().await
                }
            }
            #[cfg(test)]
            mod source_waits {
                #[cfg(test)]
                #[tokio::test]
                async fn actual_attachment_store_wait_never_substitutes_a_new_document_owner_generation_or_file()
                 {
                    orch_api_documents::routes::documents::references::source_waits::__api_run_actual_attachment_store_wait_never_substitutes_a_new_document_owner_generation_or_file().await
                }
                #[cfg(test)]
                #[tokio::test]
                async fn actual_attachment_store_wait_keeps_the_initial_raw_access_deadline_even_if_storage_extends_it()
                 {
                    orch_api_documents::routes::documents::references::source_waits::__api_run_actual_attachment_store_wait_keeps_the_initial_raw_access_deadline_even_if_storage_extends_it().await
                }
                #[cfg(test)]
                #[tokio::test]
                async fn actual_chat_attachment_wait_keeps_the_original_message_ordinal_and_material_digest()
                 {
                    orch_api_documents::routes::documents::references::source_waits::__api_run_actual_chat_attachment_wait_keeps_the_original_message_ordinal_and_material_digest().await
                }
            }
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[tokio::test]
                async fn file_cards_show_their_owner_size_and_lines_only_to_current_readers() {
                    orch_api_documents::routes::documents::references::tests::__api_run_file_cards_show_their_owner_size_and_lines_only_to_current_readers().await
                }
                #[cfg(test)]
                #[tokio::test]
                async fn an_environment_file_card_uses_its_file_permission_and_can_show_a_kept_text_excerpt()
                 {
                    orch_api_documents::routes::documents::references::tests::__api_run_an_environment_file_card_uses_its_file_permission_and_can_show_a_kept_text_excerpt().await
                }
            }
        }
        #[cfg(test)]
        mod text_creation {
            #[cfg(test)]
            mod replay_tests {
                #[cfg(test)]
                #[tokio::test]
                async fn completed_creation_uses_new_credential_current_read_and_native_metadata_without_spent_sources()
                -> ::orch_api_documents::routes::documents::text_creation::replay_tests::TestResult
                {
                    orch_api_documents::routes::documents::text_creation::replay_tests::__api_run_completed_creation_uses_new_credential_current_read_and_native_metadata_without_spent_sources().await
                }
                #[cfg(test)]
                #[tokio::test]
                async fn completed_creation_without_own_operation_read_returns_no_full_body_or_second_creation()
                -> ::orch_api_documents::routes::documents::text_creation::replay_tests::TestResult
                {
                    orch_api_documents::routes::documents::text_creation::replay_tests::__api_run_completed_creation_without_own_operation_read_returns_no_full_body_or_second_creation().await
                }
                #[cfg(test)]
                #[tokio::test]
                async fn completed_creation_replay_is_bound_to_record_account_and_current_delivery_deadline()
                -> ::orch_api_documents::routes::documents::text_creation::replay_tests::TestResult
                {
                    orch_api_documents::routes::documents::text_creation::replay_tests::__api_run_completed_creation_replay_is_bound_to_record_account_and_current_delivery_deadline().await
                }
            }
        }

        pub mod page {

            pub use orch_api_documents::routes::documents::page::get;

            pub use orch_api_documents::routes::documents::page::update;
        }
    }

    pub mod summaries {

        pub use orch_api_documents::routes::summaries::mount;
        #[cfg(test)]
        mod follow {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn future_queue_waits_for_its_actual_due_time_and_eligible_work_uses_the_configured_pace()
                 {
                    orch_api_documents::routes::summaries::follow::tests::__api_run_future_queue_waits_for_its_actual_due_time_and_eligible_work_uses_the_configured_pace()
                }
            }
        }
    }

    pub mod resources {

        pub use orch_api_resources::routes::resources::NAME_MAX_CHARS;

        pub use orch_api_base::routes::resources::LIST_LIMIT;

        pub use orch_api_resources::routes::resources::mount;

        pub use orch_api_resources::routes::resources::clean_name;

        pub use orch_api_resources::routes::resources::require;

        pub use orch_api_base::routes::resources::Home;

        pub use orch_api_resources::routes::resources::record;

        pub use orch_api_resources::routes::resources::engine_failed;

        pub mod agents {

            pub use orch_api_resources::routes::resources::agents::mount;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn a_builtin_adapter_requires_a_reported_allowed_program_but_a_self_contained_adapter_does_not()
                 {
                    orch_api_resources::routes::resources::agents::tests::__api_run_a_builtin_adapter_requires_a_reported_allowed_program_but_a_self_contained_adapter_does_not()
                }
                #[cfg(test)]
                #[test]
                fn the_system_overrides_a_member_of_the_descriptor_and_says_which() {
                    orch_api_resources::routes::resources::agents::tests::__api_run_the_system_overrides_a_member_of_the_descriptor_and_says_which()
                }
                #[cfg(test)]
                #[test]
                fn a_profile_has_four_members_and_the_same_home_choice_on_every_machine() {
                    orch_api_resources::routes::resources::agents::tests::__api_run_a_profile_has_four_members_and_the_same_home_choice_on_every_machine()
                }
                #[cfg(test)]
                #[test]
                fn a_harness_kind_is_what_an_adapter_names_itself() {
                    orch_api_resources::routes::resources::agents::tests::__api_run_a_harness_kind_is_what_an_adapter_names_itself()
                }
            }
        }

        pub mod context {

            pub use orch_api_base::routes::resources::context::Request;

            pub use orch_api_base::routes::resources::context::Resource;

            pub use orch_api_base::routes::resources::context::Projection;

            pub use orch_api_base::routes::resources::context::ContextFacts;

            pub use orch_api_base::routes::resources::context::Prepared;

            pub use orch_api_resources::routes::resources::context::facts;

            pub use orch_api_resources::routes::resources::context::prepare;
            #[cfg(test)]
            #[allow(
                clippy::unwrap_used,
                clippy::expect_used,
                reason = "a deciding case fails by panicking"
            )]
            mod tests {
                #[cfg(test)]
                #[test]
                fn every_context_read_keeps_one_business_selection_and_both_real_independent_sources()
                 {
                    orch_api_resources::routes::resources::context::tests::__api_run_every_context_read_keeps_one_business_selection_and_both_real_independent_sources()
                }
                #[cfg(test)]
                #[test]
                fn an_ordinary_home_or_name_fact_is_not_a_context_share() {
                    orch_api_resources::routes::resources::context::tests::__api_run_an_ordinary_home_or_name_fact_is_not_a_context_share()
                }
                #[cfg(test)]
                #[test]
                fn the_actual_context_projection_never_serializes_home_owner_machine_profile_usage_or_other_work()
                 {
                    orch_api_resources::routes::resources::context::tests::__api_run_the_actual_context_projection_never_serializes_home_owner_machine_profile_usage_or_other_work()
                }
            }
        }

        pub mod hosts {

            pub use orch_api_resources::routes::resources::hosts::mount;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn amounts_are_read_as_a_node_reports_them_or_as_the_api_names_them() {
                    orch_api_resources::routes::resources::hosts::tests::__api_run_amounts_are_read_as_a_node_reports_them_or_as_the_api_names_them()
                }
                #[cfg(test)]
                #[test]
                fn what_a_node_reports_of_its_drivers_and_storages_is_shown_and_what_is_not_readable_is_left_out()
                 {
                    orch_api_resources::routes::resources::hosts::tests::__api_run_what_a_node_reports_of_its_drivers_and_storages_is_shown_and_what_is_not_readable_is_left_out()
                }
                #[cfg(test)]
                #[test]
                fn labels_are_trimmed_kept_once_and_bounded() {
                    orch_api_resources::routes::resources::hosts::tests::__api_run_labels_are_trimmed_kept_once_and_bounded()
                }
            }
        }

        pub mod machines {

            pub use orch_api_resources::routes::resources::machines::mount;

            pub use orch_api_resources::routes::resources::machines::is_owner;

            pub use orch_api_resources::routes::resources::machines::machine_views;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn changed_driver_connections_report_every_schema_fault_without_echoing_values() {
                    orch_api_resources::routes::resources::machines::tests::__api_run_changed_driver_connections_report_every_schema_fault_without_echoing_values()
                }
                #[cfg(test)]
                #[test]
                fn role_and_program_controls_follow_the_one_policy_before_the_next_machine_report()
                -> Result<(), ::orch_link_contract::policy::PolicyError> {
                    orch_api_resources::routes::resources::machines::tests::__api_run_role_and_program_controls_follow_the_one_policy_before_the_next_machine_report()
                }
                #[cfg(test)]
                #[test]
                fn example_or_missing_addresses_name_the_setting_and_prevent_minting_commands() {
                    orch_api_resources::routes::resources::machines::tests::__api_run_example_or_missing_addresses_name_the_setting_and_prevent_minting_commands()
                }
                #[cfg(test)]
                #[test]
                fn enrollment_metadata_names_an_uninstallable_system_before_a_token_is_issued() {
                    orch_api_resources::routes::resources::machines::tests::__api_run_enrollment_metadata_names_an_uninstallable_system_before_a_token_is_issued()
                }
                #[cfg(test)]
                #[test]
                fn enrollment_accepts_https_and_the_installers_loopback_http_exception() {
                    orch_api_resources::routes::resources::machines::tests::__api_run_enrollment_accepts_https_and_the_installers_loopback_http_exception()
                }
                #[cfg(test)]
                #[test]
                fn minted_commands_follow_the_installers_shared_address_cases() {
                    orch_api_resources::routes::resources::machines::tests::__api_run_minted_commands_follow_the_installers_shared_address_cases()
                }
                #[cfg(test)]
                #[test]
                fn the_commands_that_enroll_a_machine_carry_the_token_and_the_address_of_the_system()
                 {
                    orch_api_resources::routes::resources::machines::tests::__api_run_the_commands_that_enroll_a_machine_carry_the_token_and_the_address_of_the_system()
                }
                #[cfg(test)]
                #[test]
                fn an_address_with_a_quote_or_a_shell_character_stays_one_word_in_every_shell() {
                    orch_api_resources::routes::resources::machines::tests::__api_run_an_address_with_a_quote_or_a_shell_character_stays_one_word_in_every_shell()
                }
                #[cfg(test)]
                #[test]
                fn a_host_installer_uses_the_first_advertised_address_in_the_systems_order() {
                    orch_api_resources::routes::resources::machines::tests::__api_run_a_host_installer_uses_the_first_advertised_address_in_the_systems_order()
                }
                #[cfg(test)]
                #[test]
                fn a_key_the_owner_handed_over_singly_or_all_at_once_may_be_changed_from_here_and_the_handing_over_never()
                 {
                    orch_api_resources::routes::resources::machines::tests::__api_run_a_key_the_owner_handed_over_singly_or_all_at_once_may_be_changed_from_here_and_the_handing_over_never()
                }
            }
        }

        pub mod mcp {

            pub use orch_api_resources::routes::resources::mcp::mount;
            #[cfg(test)]
            mod delete {
                #[cfg(test)]
                mod machine_tests {
                    #[cfg(test)]
                    #[tokio::test]
                    async fn native_machine_deletion_uses_real_canonical_facets_and_queues_owned_custody_on_the_same_commit() -> ::orch_api_resources::routes::resources::mcp::delete::machine_tests::TestResult{
                        orch_api_resources::routes::resources::mcp::delete::machine_tests::__api_run_native_machine_deletion_uses_real_canonical_facets_and_queues_owned_custody_on_the_same_commit().await
                    }
                    #[cfg(test)]
                    #[tokio::test]
                    async fn native_machine_deletion_refuses_presets_and_another_machines_canonical_import() -> ::orch_api_resources::routes::resources::mcp::delete::machine_tests::TestResult{
                        orch_api_resources::routes::resources::mcp::delete::machine_tests::__api_run_native_machine_deletion_refuses_presets_and_another_machines_canonical_import().await
                    }
                    #[cfg(test)]
                    #[tokio::test]
                    async fn a_retained_ready_copy_does_not_let_its_machine_forget_another_machines_canonical_import() -> ::orch_api_resources::routes::resources::mcp::delete::machine_tests::TestResult{
                        orch_api_resources::routes::resources::mcp::delete::machine_tests::__api_run_a_retained_ready_copy_does_not_let_its_machine_forget_another_machines_canonical_import().await
                    }
                    #[cfg(test)]
                    #[tokio::test]
                    async fn native_machine_deletion_reloads_current_owner_after_the_actual_authority_wait() -> ::orch_api_resources::routes::resources::mcp::delete::machine_tests::TestResult{
                        orch_api_resources::routes::resources::mcp::delete::machine_tests::__api_run_native_machine_deletion_reloads_current_owner_after_the_actual_authority_wait().await
                    }
                }
            }
        }

        pub mod models {

            pub use orch_api_resources::routes::resources::models::mount;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn a_routing_name_is_taken_as_it_is_given_and_a_display_name_is_trimmed() {
                    orch_api_resources::routes::resources::models::tests::__api_run_a_routing_name_is_taken_as_it_is_given_and_a_display_name_is_trimmed()
                }
                #[cfg(test)]
                #[test]
                fn a_default_has_to_be_among_what_is_enabled_and_a_context_within_what_is_open() {
                    orch_api_resources::routes::resources::models::tests::__api_run_a_default_has_to_be_among_what_is_enabled_and_a_context_within_what_is_open()
                }
                #[cfg(test)]
                #[test]
                fn reasoning_starts_require_enablement_and_suggestions_only_offer_them() {
                    orch_api_resources::routes::resources::models::tests::__api_run_reasoning_starts_require_enablement_and_suggestions_only_offer_them()
                }
                #[cfg(test)]
                #[test]
                fn capacity_starts_require_enablement_and_suggestions_never_open_capacity() {
                    orch_api_resources::routes::resources::models::tests::__api_run_capacity_starts_require_enablement_and_suggestions_never_open_capacity()
                }
                #[cfg(test)]
                #[test]
                fn accepting_a_cache_lifetime_suggestion_only_offers_the_additional_lifetime() {
                    orch_api_resources::routes::resources::models::tests::__api_run_accepting_a_cache_lifetime_suggestion_only_offers_the_additional_lifetime()
                }
                #[cfg(test)]
                #[test]
                fn group_names_are_deduplicated_and_the_current_configured_count_is_independent() {
                    orch_api_resources::routes::resources::models::tests::__api_run_group_names_are_deduplicated_and_the_current_configured_count_is_independent()
                }
                #[cfg(test)]
                #[test]
                fn a_larger_context_that_a_suggestion_lists_is_on_offer_and_no_start_comes_from_discovery()
                 {
                    orch_api_resources::routes::resources::models::tests::__api_run_a_larger_context_that_a_suggestion_lists_is_on_offer_and_no_start_comes_from_discovery()
                }
                #[cfg(test)]
                #[test]
                fn accepting_a_report_without_enabled_options_preserves_its_future_facts() {
                    orch_api_resources::routes::resources::models::tests::__api_run_accepting_a_report_without_enabled_options_preserves_its_future_facts()
                }
                #[cfg(test)]
                #[test]
                fn what_a_suggestion_lists_is_on_offer_and_the_organizations_choice_switches_it_on()
                {
                    orch_api_resources::routes::resources::models::tests::__api_run_what_a_suggestion_lists_is_on_offer_and_the_organizations_choice_switches_it_on()
                }
                #[cfg(test)]
                #[test]
                fn search_offers_never_enable_the_tool_or_its_default() {
                    orch_api_resources::routes::resources::models::tests::__api_run_search_offers_never_enable_the_tool_or_its_default()
                }
                #[cfg(test)]
                #[test]
                fn a_model_without_a_window_of_its_own_has_the_largest_context_its_source_reports()
                {
                    orch_api_resources::routes::resources::models::tests::__api_run_a_model_without_a_window_of_its_own_has_the_largest_context_its_source_reports()
                }
                #[cfg(test)]
                #[test]
                fn a_name_the_catalogue_lists_is_added_with_what_the_catalogue_says_under_what_was_written()
                 {
                    orch_api_resources::routes::resources::models::tests::__api_run_a_name_the_catalogue_lists_is_added_with_what_the_catalogue_says_under_what_was_written()
                }
                #[cfg(test)]
                #[test]
                fn the_organizations_choice_of_what_adding_switches_on_is_read_with_all_as_the_shipped_one()
                 {
                    orch_api_resources::routes::resources::models::tests::__api_run_the_organizations_choice_of_what_adding_switches_on_is_read_with_all_as_the_shipped_one()
                }
            }
        }

        pub mod mounts {

            pub use orch_api_resources::routes::resources::mounts::mount;
            #[cfg(test)]
            pub(crate) mod creation {
                #[cfg(test)]
                mod replay_tests {
                    #[cfg(test)]
                    #[tokio::test]
                    async fn singleton_completed_replay_uses_new_credential_native_current_style_and_spent_effects_are_absent() -> ::orch_api_resources::routes::resources::mounts::creation::replay_tests::Proof{
                        orch_api_resources::routes::resources::mounts::creation::replay_tests::__api_run_singleton_completed_replay_uses_new_credential_native_current_style_and_spent_effects_are_absent().await
                    }
                    #[cfg(test)]
                    #[tokio::test]
                    async fn singleton_completed_replay_keeps_receipts_account_boundary_and_current_publication_cut() -> ::orch_api_resources::routes::resources::mounts::creation::replay_tests::Proof{
                        orch_api_resources::routes::resources::mounts::creation::replay_tests::__api_run_singleton_completed_replay_keeps_receipts_account_boundary_and_current_publication_cut().await
                    }
                    #[cfg(test)]
                    #[tokio::test]
                    async fn singleton_auxiliary_read_uses_current_output_scope_after_native_locks_once() -> ::orch_api_resources::routes::resources::mounts::creation::replay_tests::Proof{
                        orch_api_resources::routes::resources::mounts::creation::replay_tests::__api_run_singleton_auxiliary_read_uses_current_output_scope_after_native_locks_once().await
                    }
                    #[cfg(test)]
                    #[tokio::test]
                    async fn singleton_auxiliary_read_keeps_first_sql_lifetime_and_does_not_run_without_response_read() -> ::orch_api_resources::routes::resources::mounts::creation::replay_tests::Proof{
                        orch_api_resources::routes::resources::mounts::creation::replay_tests::__api_run_singleton_auxiliary_read_keeps_first_sql_lifetime_and_does_not_run_without_response_read().await
                    }
                }
            }
            #[cfg(test)]
            mod read {
                #[cfg(test)]
                mod tests {
                    #[cfg(test)]
                    #[tokio::test]
                    async fn snapshot_get_prepares_its_native_projection_once_for_each_http_credential()
                     {
                        orch_api_resources::routes::resources::mounts::read::tests::__api_run_snapshot_get_prepares_its_native_projection_once_for_each_http_credential().await
                    }
                    #[cfg(test)]
                    #[tokio::test]
                    async fn a_declared_snapshot_reader_cannot_return_content_without_its_actual_read_guard()
                     {
                        orch_api_resources::routes::resources::mounts::read::tests::__api_run_a_declared_snapshot_reader_cannot_return_content_without_its_actual_read_guard().await
                    }
                    #[cfg(test)]
                    #[tokio::test]
                    async fn actual_session_legacy_and_scoped_get_list_hold_native_bodies_and_errors_until_postcommit_deadline()
                     {
                        orch_api_resources::routes::resources::mounts::read::tests::__api_run_actual_session_legacy_and_scoped_get_list_hold_native_bodies_and_errors_until_postcommit_deadline().await
                    }
                    #[cfg(test)]
                    #[tokio::test]
                    async fn actual_native_get_auxiliary_hook_runs_once_outside_retry_and_keeps_errors_behind_the_final_cut()
                     {
                        orch_api_resources::routes::resources::mounts::read::tests::__api_run_actual_native_get_auxiliary_hook_runs_once_outside_retry_and_keeps_errors_behind_the_final_cut().await
                    }
                    #[cfg(test)]
                    #[tokio::test]
                    async fn native_get_context_uses_actual_session_mode_and_the_same_principal_time_after_opening()
                     {
                        orch_api_resources::routes::resources::mounts::read::tests::__api_run_native_get_context_uses_actual_session_mode_and_the_same_principal_time_after_opening().await
                    }
                }
            }
            #[cfg(test)]
            mod volume {
                #[cfg(test)]
                mod tests {
                    #[cfg(test)]
                    #[tokio::test]
                    async fn native_volume_preparation_freezes_the_actual_driver_storage_and_owner_labels_without_issuing()
                     {
                        orch_api_resources::routes::resources::mounts::volume::tests::__api_run_native_volume_preparation_freezes_the_actual_driver_storage_and_owner_labels_without_issuing().await
                    }
                    #[cfg(test)]
                    #[tokio::test]
                    async fn native_volume_first_queue_cut_refuses_before_insert_and_positive_control_retains_one_exact_command()
                     {
                        orch_api_resources::routes::resources::mounts::volume::tests::__api_run_native_volume_first_queue_cut_refuses_before_insert_and_positive_control_retains_one_exact_command().await
                    }
                }
            }
        }
        #[cfg(test)]
        pub(crate) mod projection {
            #[cfg(test)]
            pub(crate) mod single {
                #[cfg(test)]
                mod tests {
                    #[cfg(test)]
                    #[tokio::test]
                    async fn document_cut_rejects_a_changed_hierarchy_after_snapshot_commit() {
                        orch_api_resources::routes::resources::projection::single::tests::__api_run_document_cut_rejects_a_changed_hierarchy_after_snapshot_commit().await
                    }
                    #[cfg(test)]
                    #[tokio::test]
                    async fn folder_cut_rejects_a_changed_membership_generation_after_snapshot_commit()
                     {
                        orch_api_resources::routes::resources::projection::single::tests::__api_run_folder_cut_rejects_a_changed_membership_generation_after_snapshot_commit().await
                    }
                    #[cfg(test)]
                    #[tokio::test]
                    async fn document_metadata_finisher_keeps_read_after_completed_manage_is_removed()
                     {
                        orch_api_resources::routes::resources::projection::single::tests::__api_run_document_metadata_finisher_keeps_read_after_completed_manage_is_removed().await
                    }
                    #[cfg(test)]
                    #[tokio::test]
                    async fn folder_metadata_finisher_keeps_read_after_completed_manage_is_removed()
                    {
                        orch_api_resources::routes::resources::projection::single::tests::__api_run_folder_metadata_finisher_keeps_read_after_completed_manage_is_removed().await
                    }
                    #[cfg(test)]
                    #[tokio::test]
                    async fn document_metadata_finisher_withholds_read_lost_after_commit() {
                        orch_api_resources::routes::resources::projection::single::tests::__api_run_document_metadata_finisher_withholds_read_lost_after_commit().await
                    }
                    #[cfg(test)]
                    #[tokio::test]
                    async fn folder_metadata_finisher_withholds_read_lost_after_commit() {
                        orch_api_resources::routes::resources::projection::single::tests::__api_run_folder_metadata_finisher_withholds_read_lost_after_commit().await
                    }
                    #[cfg(test)]
                    #[tokio::test]
                    async fn a_key_expiring_after_metadata_commit_keeps_only_its_accepted_receipt()
                    {
                        orch_api_resources::routes::resources::projection::single::tests::__api_run_a_key_expiring_after_metadata_commit_keeps_only_its_accepted_receipt().await
                    }
                }
            }
        }

        pub mod provider_login {

            pub use orch_api_resources::routes::resources::provider_login::mount;

            pub use orch_api_resources::routes::resources::provider_login::secret_fields_of;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn what_a_person_typed_is_read_as_the_type_of_the_field_it_was_typed_for() {
                    orch_api_resources::routes::resources::provider_login::tests::__api_run_what_a_person_typed_is_read_as_the_type_of_the_field_it_was_typed_for()
                }
                #[cfg(test)]
                #[test]
                fn each_value_that_does_not_fit_its_field_is_named_by_the_field() {
                    orch_api_resources::routes::resources::provider_login::tests::__api_run_each_value_that_does_not_fit_its_field_is_named_by_the_field()
                }
                #[cfg(test)]
                #[test]
                fn a_text_longer_than_a_field_takes_is_refused_and_a_secret_field_is_known_by_its_type()
                 {
                    orch_api_resources::routes::resources::provider_login::tests::__api_run_a_text_longer_than_a_field_takes_is_refused_and_a_secret_field_is_known_by_its_type()
                }
                #[cfg(test)]
                #[test]
                fn a_sign_in_nobody_began_is_shown_as_none_running() {
                    orch_api_resources::routes::resources::provider_login::tests::__api_run_a_sign_in_nobody_began_is_shown_as_none_running()
                }
            }
        }
        #[cfg(test)]
        mod provider_services {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn editing_a_current_service_preserves_only_unchanged_stale_offerings_and_units()
                -> Result<(), String> {
                    orch_api_resources::routes::resources::provider_services::tests::__api_run_editing_a_current_service_preserves_only_unchanged_stale_offerings_and_units()
                }
            }
        }
        #[cfg(test)]
        pub(crate) mod provider_sources {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn only_a_registered_program_can_present_a_key_and_no_source_can_present_a_sign_in()
                {
                    orch_api_resources::routes::resources::provider_sources::tests::__api_run_only_a_registered_program_can_present_a_key_and_no_source_can_present_a_sign_in()
                }
                #[cfg(test)]
                #[test]
                fn the_source_address_obeys_the_same_plain_http_policy_as_an_endpoint() {
                    orch_api_resources::routes::resources::provider_sources::tests::__api_run_the_source_address_obeys_the_same_plain_http_policy_as_an_endpoint()
                }
                #[cfg(test)]
                #[test]
                fn a_sign_in_cannot_be_opted_into_an_address_and_a_program_from_another_machine_is_refused()
                 {
                    orch_api_resources::routes::resources::provider_sources::tests::__api_run_a_sign_in_cannot_be_opted_into_an_address_and_a_program_from_another_machine_is_refused()
                }
            }
        }

        pub mod providers {

            pub use orch_api_resources::routes::resources::providers::mount;

            pub use orch_api_resources::routes::resources::providers::refused_making;

            pub use orch_api_resources::routes::resources::providers::refresh_standing;

            pub use orch_api_resources::routes::resources::providers::model_view;

            pub use orch_api_resources::routes::resources::providers::show;

            pub use orch_api_resources::routes::resources::providers::provider_for;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn a_provider_with_a_credential_and_a_model_is_ready_and_without_either_it_has_to_be_set_up()
                 {
                    orch_api_resources::routes::resources::providers::tests::__api_run_a_provider_with_a_credential_and_a_model_is_ready_and_without_either_it_has_to_be_set_up()
                }
                #[cfg(test)]
                #[test]
                fn a_copy_of_the_credential_leaving_is_said_and_staying_home_is_not() {
                    orch_api_resources::routes::resources::providers::tests::__api_run_a_copy_of_the_credential_leaving_is_said_and_staying_home_is_not()
                }
                #[cfg(test)]
                #[test]
                fn a_provider_allows_at_least_one_type_and_each_type_once() {
                    orch_api_resources::routes::resources::providers::tests::__api_run_a_provider_allows_at_least_one_type_and_each_type_once()
                }
                #[cfg(test)]
                #[test]
                fn a_runtime_has_values_for_the_keys_of_its_own_scope_only() {
                    orch_api_resources::routes::resources::providers::tests::__api_run_a_runtime_has_values_for_the_keys_of_its_own_scope_only()
                }
                #[cfg(test)]
                #[test]
                fn old_kept_credit_overrides_keep_their_precedence_as_balances_without_legacy_members_in_the_answer()
                 {
                    orch_api_resources::routes::resources::providers::tests::__api_run_old_kept_credit_overrides_keep_their_precedence_as_balances_without_legacy_members_in_the_answer()
                }
                #[cfg(test)]
                #[test]
                fn manual_balance_and_spend_amounts_keep_decimal_precision_and_refuse_exponent_or_non_decimal_text()
                 {
                    orch_api_resources::routes::resources::providers::tests::__api_run_manual_balance_and_spend_amounts_keep_decimal_precision_and_refuse_exponent_or_non_decimal_text()
                }
            }
        }

        pub mod setup {

            pub use orch_api_resources::routes::resources::setup::mount;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn a_profile_that_has_reported_no_descriptor_declares_nothing_yet() {
                    orch_api_resources::routes::resources::setup::tests::__api_run_a_profile_that_has_reported_no_descriptor_declares_nothing_yet()
                }
                #[cfg(test)]
                #[test]
                fn presets_are_the_vendors_endpoints_by_name_and_one_set_to_null_is_hidden() {
                    orch_api_resources::routes::resources::setup::tests::__api_run_presets_are_the_vendors_endpoints_by_name_and_one_set_to_null_is_hidden()
                }
                #[cfg(test)]
                #[test]
                fn preset_key_hints_are_optional_public_metadata_and_keep_future_members() {
                    orch_api_resources::routes::resources::setup::tests::__api_run_preset_key_hints_are_optional_public_metadata_and_keep_future_members()
                }
                #[cfg(test)]
                #[test]
                fn a_key_only_offers_the_types_in_which_it_stays_home_by_default() {
                    orch_api_resources::routes::resources::setup::tests::__api_run_a_key_only_offers_the_types_in_which_it_stays_home_by_default()
                }
                #[cfg(test)]
                #[test]
                fn a_native_reader_without_a_proposal_uses_only_the_supported_creation_default() {
                    orch_api_resources::routes::resources::setup::tests::__api_run_a_native_reader_without_a_proposal_uses_only_the_supported_creation_default()
                }
            }
        }

        pub mod skills {

            pub use orch_api_resources::routes::resources::skills::mount;
            #[cfg(test)]
            pub(crate) mod draft_creation {
                #[cfg(test)]
                mod tests {
                    #[cfg(test)]
                    #[tokio::test]
                    async fn actual_new_skill_draft_keeps_current_http_deadline_after_blob_header_and_draft_file_waits()
                     {
                        orch_api_resources::routes::resources::skills::draft_creation::tests::__api_run_actual_new_skill_draft_keeps_current_http_deadline_after_blob_header_and_draft_file_waits().await
                    }
                }
            }
            #[cfg(test)]
            mod source_url {
                #[cfg(test)]
                mod stage_tests {
                    #[cfg(test)]
                    #[tokio::test]
                    async fn native_source_sealing_prepares_without_custody_and_consumes_one_real_first_insert_cut()
                     {
                        orch_api_resources::routes::resources::skills::source_url::stage_tests::__api_run_native_source_sealing_prepares_without_custody_and_consumes_one_real_first_insert_cut().await
                    }
                    #[cfg(test)]
                    #[tokio::test]
                    async fn native_source_public_noop_actor_and_transaction_rules_do_not_fabricate_secret_or_first_effect()
                     {
                        orch_api_resources::routes::resources::skills::source_url::stage_tests::__api_run_native_source_public_noop_actor_and_transaction_rules_do_not_fabricate_secret_or_first_effect().await
                    }
                }
                #[cfg(test)]
                mod tests {
                    #[cfg(test)]
                    #[test]
                    fn a_source_address_hides_userinfo_queries_and_fragments() {
                        orch_api_resources::routes::resources::skills::source_url::tests::__api_run_a_source_address_hides_userinfo_queries_and_fragments()
                    }
                }
            }
        }

        pub mod templates {

            pub use orch_api_resources::routes::resources::templates::mount;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn a_driver_is_named_like_a_program_and_a_spec_is_an_object() {
                    orch_api_resources::routes::resources::templates::tests::__api_run_a_driver_is_named_like_a_program_and_a_spec_is_an_object()
                }
            }
        }

        pub mod upstreams {

            pub use orch_api_resources::routes::resources::upstreams::PROTOCOLS;

            pub use orch_api_resources::routes::resources::upstreams::Checked;

            pub use orch_api_resources::routes::resources::upstreams::check;

            pub use orch_api_resources::routes::resources::upstreams::stored;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn an_endpoint_on_the_runtimes_own_loopback_needs_no_tls_whatever_the_policy() {
                    orch_api_resources::routes::resources::upstreams::tests::__api_run_an_endpoint_on_the_runtimes_own_loopback_needs_no_tls_whatever_the_policy()
                }
                #[cfg(test)]
                #[test]
                fn an_endpoint_is_https_and_an_upstream_has_one_for_each_protocol() {
                    orch_api_resources::routes::resources::upstreams::tests::__api_run_an_endpoint_is_https_and_an_upstream_has_one_for_each_protocol()
                }
                #[cfg(test)]
                #[test]
                fn only_the_options_a_protocol_declares_are_taken_and_the_key_header_is_the_forwarders()
                 {
                    orch_api_resources::routes::resources::upstreams::tests::__api_run_only_the_options_a_protocol_declares_are_taken_and_the_key_header_is_the_forwarders()
                }
                #[cfg(test)]
                #[test]
                fn an_upstream_that_takes_no_key_on_any_endpoint_is_keyless_and_secret_headers_are_collected()
                 {
                    orch_api_resources::routes::resources::upstreams::tests::__api_run_an_upstream_that_takes_no_key_on_any_endpoint_is_keyless_and_secret_headers_are_collected()
                }
                #[cfg(test)]
                #[test]
                fn declared_model_defaults_survive_save_in_the_runtime_endpoint_shape() {
                    orch_api_resources::routes::resources::upstreams::tests::__api_run_declared_model_defaults_survive_save_in_the_runtime_endpoint_shape()
                }
                #[cfg(test)]
                #[test]
                fn own_chat_output_limit_choices_survive_save_and_unknown_fields_are_refused() {
                    orch_api_resources::routes::resources::upstreams::tests::__api_run_own_chat_output_limit_choices_survive_save_and_unknown_fields_are_refused()
                }
                #[cfg(test)]
                #[test]
                fn model_defaults_use_shared_object_and_owned_field_validation() {
                    orch_api_resources::routes::resources::upstreams::tests::__api_run_model_defaults_use_shared_object_and_owned_field_validation()
                }
            }
        }

        pub mod page {

            pub use orch_api_resources::routes::resources::page::get;

            pub use orch_api_resources::routes::resources::page::update;
        }

        pub mod host_delete {

            pub use orch_api_resources::routes::resources::host_delete::mount;
        }

        pub mod provider_runtime {

            pub use orch_api_resources::routes::resources::provider_runtime::mount;
        }

        pub mod host_storage {

            pub use orch_api_resources::routes::resources::host_storage::mount;
        }

        pub mod provider_updates {

            pub use orch_api_resources::routes::resources::provider_updates::mount;
        }

        pub mod sealing {

            pub use orch_api_resources::routes::resources::sealing::SECRET_MAX_BYTES;

            pub use orch_api_resources::routes::resources::sealing::PROVIDER;

            pub use orch_api_resources::routes::resources::sealing::SKILL;

            pub use orch_api_resources::routes::resources::sealing::API_KEY;

            pub use orch_api_resources::routes::resources::sealing::HEADER_PREFIX;

            pub use orch_api_resources::routes::resources::sealing::install_master_key;

            pub use orch_api_resources::routes::resources::sealing::binding;

            pub use orch_api_resources::routes::resources::sealing::header_secret;

            pub use orch_api_resources::routes::resources::sealing::put_provider_secret;

            pub use orch_api_resources::routes::resources::sealing::put_skill_source_secret;
        }

        pub mod host_maintenance {

            pub use orch_api_resources::routes::resources::host_maintenance::mount;
        }

        pub mod mcp_readers {}

        pub mod provider_resets {

            pub use orch_api_resources::routes::resources::provider_resets::mount;

            pub use orch_api_resources::routes::resources::provider_resets::last_resets;
        }

        pub mod library {

            pub use orch_api_resources::routes::resources::library::mount;
        }

        pub mod provider_plan {

            pub use orch_api_resources::routes::resources::provider_plan::add_forecasts;

            pub use orch_api_resources::routes::resources::provider_plan::get;
        }

        pub mod impact {

            pub use orch_api_resources::routes::resources::impact::mount;
        }

        pub mod view {

            pub use orch_api_base::routes::resources::view::parse;

            pub use orch_api_resources::routes::resources::view::is_online;

            pub use orch_api_resources::routes::resources::view::machine_refs;

            pub use orch_api_resources::routes::resources::view::addresses;

            pub use orch_api_resources::routes::resources::view::provider_refs;

            pub use orch_api_resources::routes::resources::view::agent_refs;

            pub use orch_api_resources::routes::resources::view::environment_refs;

            pub use orch_api_resources::routes::resources::view::environment_activity;

            pub use orch_api_base::routes::resources::view::Holding;

            pub use orch_api_base::routes::resources::view::accesses;
        }
    }

    pub mod environments {

        pub use orch_api_environments::routes::environments::mount;
        #[cfg(test)]
        mod agent_reach {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn each_threads_line_names_its_arrangement_and_only_a_copy_has_an_expiry() {
                    orch_api_environments::routes::environments::agent_reach::tests::__api_run_each_threads_line_names_its_arrangement_and_only_a_copy_has_an_expiry()
                }
            }
        }
        #[cfg(test)]
        mod exec_version {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn a_manager_approves_only_the_reported_program_and_the_server_stamps_its_writer() {
                    orch_api_environments::routes::environments::exec_version::tests::__api_run_a_manager_approves_only_the_reported_program_and_the_server_stamps_its_writer()
                }
                #[cfg(test)]
                #[test]
                fn a_missing_historical_writer_does_not_revoke_a_current_path_and_version() {
                    orch_api_environments::routes::environments::exec_version::tests::__api_run_a_missing_historical_writer_does_not_revoke_a_current_path_and_version()
                }
            }
        }
        #[cfg(test)]
        mod incarnation {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn source_members_and_deployment_key_change_the_opaque_value_without_revealing_inputs()
                -> ::orch_api_base::middleware::requests::ApiResult<()> {
                    orch_api_environments::routes::environments::incarnation::tests::__api_run_source_members_and_deployment_key_change_the_opaque_value_without_revealing_inputs()
                }
                #[cfg(test)]
                #[test]
                fn unrelated_metadata_and_home_changes_preserve_the_same_source_token()
                -> ::orch_api_base::middleware::requests::ApiResult<()> {
                    orch_api_environments::routes::environments::incarnation::tests::__api_run_unrelated_metadata_and_home_changes_preserve_the_same_source_token()
                }
                #[cfg(test)]
                #[test]
                fn an_absent_legacy_selection_remains_distinct_from_an_explicit_empty_selection()
                -> ::orch_api_base::middleware::requests::ApiResult<()> {
                    orch_api_environments::routes::environments::incarnation::tests::__api_run_an_absent_legacy_selection_remains_distinct_from_an_explicit_empty_selection()
                }
            }
        }
        #[cfg(test)]
        pub(crate) mod mcp {
            #[cfg(test)]
            mod import_sources {
                #[cfg(test)]
                mod error_tests {
                    #[cfg(test)]
                    #[test]
                    fn an_import_refusal_keeps_its_kind_without_exposing_native_details_as_an_offline_error()
                     {
                        orch_api_environments::routes::environments::mcp::import_sources::error_tests::__api_run_an_import_refusal_keeps_its_kind_without_exposing_native_details_as_an_offline_error()
                    }
                }
            }
        }
        #[cfg(test)]
        pub(crate) mod network {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn a_restricted_unit_is_never_shown_ready_before_its_host_said_so() {
                    orch_api_environments::routes::environments::network::tests::__api_run_a_restricted_unit_is_never_shown_ready_before_its_host_said_so()
                }
                #[cfg(test)]
                #[test]
                fn an_old_open_unit_that_a_restriction_was_asked_of_is_kept_stopped_and_says_why() {
                    orch_api_environments::routes::environments::network::tests::__api_run_an_old_open_unit_that_a_restriction_was_asked_of_is_kept_stopped_and_says_why()
                }
                #[cfg(test)]
                #[test]
                fn a_directory_on_a_persons_machine_has_the_machines_network() {
                    orch_api_environments::routes::environments::network::tests::__api_run_a_directory_on_a_persons_machine_has_the_machines_network()
                }
                #[cfg(test)]
                #[test]
                fn a_thread_reader_sees_the_network_mode_and_state_without_its_private_addresses() {
                    orch_api_environments::routes::environments::network::tests::__api_run_a_thread_reader_sees_the_network_mode_and_state_without_its_private_addresses()
                }
            }
        }
        #[cfg(test)]
        mod support {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn an_environment_whose_facts_hold_nothing_of_it_was_found_to_have_nothing() {
                    orch_api_environments::routes::environments::support::tests::__api_run_an_environment_whose_facts_hold_nothing_of_it_was_found_to_have_nothing()
                }
                #[cfg(test)]
                #[test]
                fn what_the_facts_hold_is_told_as_it_was_found() {
                    orch_api_environments::routes::environments::support::tests::__api_run_what_the_facts_hold_is_told_as_it_was_found()
                }
                #[cfg(test)]
                #[test]
                fn one_odd_entry_does_not_hide_the_others() {
                    orch_api_environments::routes::environments::support::tests::__api_run_one_odd_entry_does_not_hide_the_others()
                }
            }
        }
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn existing_config_previews_validate_full_consent_and_patch_deferral_is_narrow() {
                orch_api_environments::routes::environments::tests::__api_run_existing_config_previews_validate_full_consent_and_patch_deferral_is_narrow()
            }
            #[cfg(test)]
            #[test]
            fn a_root_is_an_absolute_path_of_the_machines_own_kind_that_does_not_climb() {
                orch_api_environments::routes::environments::tests::__api_run_a_root_is_an_absolute_path_of_the_machines_own_kind_that_does_not_climb()
            }
            #[cfg(test)]
            #[test]
            fn a_root_must_lie_inside_a_root_the_machine_lists_and_a_machine_that_lists_none_is_asked()
             {
                orch_api_environments::routes::environments::tests::__api_run_a_root_must_lie_inside_a_root_the_machine_lists_and_a_machine_that_lists_none_is_asked()
            }
            #[cfg(test)]
            #[test]
            fn what_the_exec_node_offers_is_what_is_configured_within_what_the_machine_offers() {
                orch_api_environments::routes::environments::tests::__api_run_what_the_exec_node_offers_is_what_is_configured_within_what_the_machine_offers()
            }
        }
    }

    pub mod workspace {

        pub use orch_api_environments::routes::workspace::mount;

        pub use orch_api_base::routes::workspace::json_reply;

        pub mod files {

            pub use orch_api_environments::routes::workspace::files::mount;

            pub use orch_api_environments::routes::workspace::files::serve;

            pub use orch_api_environments::routes::workspace::files::unsatisfiable;

            pub use orch_api_environments::routes::workspace::files::range_of;

            pub use orch_api_environments::routes::workspace::files::read_for;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn a_version_has_one_identifier_for_its_fetch_and_another_for_each_new_try() {
                    orch_api_environments::routes::workspace::files::tests::__api_run_a_version_has_one_identifier_for_its_fetch_and_another_for_each_new_try()
                }
                #[cfg(test)]
                #[test]
                fn a_name_with_more_than_ascii_is_offered_to_a_download_in_both_forms() {
                    orch_api_environments::routes::workspace::files::tests::__api_run_a_name_with_more_than_ascii_is_offered_to_a_download_in_both_forms()
                }
                #[cfg(test)]
                #[test]
                fn what_a_write_was_made_on_is_a_hash_or_the_word_none() {
                    orch_api_environments::routes::workspace::files::tests::__api_run_what_a_write_was_made_on_is_a_hash_or_the_word_none()
                }
            }
        }
        #[cfg(test)]
        pub(crate) mod pages {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn bounded_unsorted_selection_never_skips_a_large_omitted_sibling()
                -> ::orch_api_environments::routes::workspace::pages::tests::Proof {
                    orch_api_environments::routes::workspace::pages::tests::__api_run_bounded_unsorted_selection_never_skips_a_large_omitted_sibling()
                }
                #[cfg(test)]
                #[test]
                fn cursor_binding_and_expiry_fail_with_an_explicit_restart()
                -> ::orch_api_environments::routes::workspace::pages::tests::Proof {
                    orch_api_environments::routes::workspace::pages::tests::__api_run_cursor_binding_and_expiry_fail_with_an_explicit_restart()
                }
                #[cfg(test)]
                #[test]
                fn native_observation_interval_is_bound_into_the_http_cursor_and_reversed_intervals_are_refused()
                -> ::orch_api_environments::routes::workspace::pages::tests::Proof {
                    orch_api_environments::routes::workspace::pages::tests::__api_run_native_observation_interval_is_bound_into_the_http_cursor_and_reversed_intervals_are_refused()
                }
            }
        }

        pub mod paths {

            pub use orch_api_environments::routes::workspace::paths::PATH_MAX_BYTES;

            pub use orch_api_environments::routes::workspace::paths::clean;

            pub use orch_api_environments::routes::workspace::paths::mount;

            pub use orch_api_environments::routes::workspace::paths::cache_key;

            pub use orch_api_environments::routes::workspace::paths::parent_of;

            pub use orch_api_environments::routes::workspace::paths::name_of;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn a_path_is_relative_and_does_not_climb() {
                    orch_api_environments::routes::workspace::paths::tests::__api_run_a_path_is_relative_and_does_not_climb()
                }
                #[cfg(test)]
                #[test]
                fn a_path_has_a_parent_and_a_name() {
                    orch_api_environments::routes::workspace::paths::tests::__api_run_a_path_has_a_parent_and_a_name()
                }
            }
        }

        pub mod problems {

            pub use orch_api_base::routes::workspace::problems::Doing;

            pub use orch_api_base::routes::workspace::problems::Failure;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn directory_capacity_refusal_is_available_to_retry_with_its_native_reason() {
                    orch_api_environments::routes::workspace::problems::tests::__api_run_directory_capacity_refusal_is_available_to_retry_with_its_native_reason()
                }
                #[cfg(test)]
                #[test]
                fn the_same_refusal_of_a_machine_is_another_problem_for_each_thing_asked() {
                    orch_api_environments::routes::workspace::problems::tests::__api_run_the_same_refusal_of_a_machine_is_another_problem_for_each_thing_asked()
                }
            }
        }

        pub mod restores {

            pub use orch_api_environments::routes::workspace::restores::mount;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn later_writer_names_follow_proved_paths_across_overlapping_directories() {
                    orch_api_environments::routes::workspace::restores::tests::__api_run_later_writer_names_follow_proved_paths_across_overlapping_directories()
                }
                #[cfg(test)]
                #[test]
                fn later_changes_compare_added_removed_link_type_and_mode_without_guessing_unknowns()
                 {
                    orch_api_environments::routes::workspace::restores::tests::__api_run_later_changes_compare_added_removed_link_type_and_mode_without_guessing_unknowns()
                }
            }
        }

        pub mod listing {

            pub use orch_api_environments::routes::workspace::listing::LISTING_MAX_ENTRIES;

            pub use orch_api_environments::routes::workspace::listing::indexed;

            pub use orch_api_environments::routes::workspace::listing::dir_entry;

            pub use orch_api_environments::routes::workspace::listing::sort;

            pub use orch_api_environments::routes::workspace::listing::from_manifest;

            pub use orch_api_environments::routes::workspace::listing::from_cache;

            pub use orch_api_environments::routes::workspace::listing::mark_cached;

            pub use orch_api_environments::routes::workspace::listing::file_stat;

            pub use orch_api_environments::routes::workspace::listing::millis_of;

            pub use orch_api_environments::routes::workspace::listing::cut;
        }

        pub mod live {

            pub use orch_api_environments::routes::workspace::live::machine_of;

            pub use orch_api_environments::routes::workspace::live::offline;

            pub use orch_api_base::routes::workspace::live::run;

            pub use orch_api_environments::routes::workspace::live::id_to_ask;

            pub use orch_api_environments::routes::workspace::live::require_live;

            pub use orch_api_environments::routes::workspace::live::refused;
        }

        pub mod snapshots {

            pub use orch_api_environments::routes::workspace::snapshots::mount;
        }

        pub mod door {

            pub use orch_api_environments::routes::workspace::door::Door;

            pub use orch_api_environments::routes::workspace::door::beside;

            pub use orch_api_environments::routes::workspace::door::open;
        }

        pub mod source {

            pub use orch_api_environments::routes::workspace::source::mount;
        }

        pub mod looking {

            pub use orch_api_environments::routes::workspace::looking::mount;
        }
    }

    pub mod admin {

        pub use orch_api_admin::routes::admin::mount;

        pub mod audit {

            pub use orch_api_admin::routes::admin::audit::mount;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn a_cell_that_a_spreadsheet_would_run_is_written_as_text_and_a_cell_with_a_comma_is_quoted()
                 {
                    orch_api_admin::routes::admin::audit::tests::__api_run_a_cell_that_a_spreadsheet_would_run_is_written_as_text_and_a_cell_with_a_comma_is_quoted()
                }
                #[cfg(test)]
                #[test]
                fn a_file_is_named_by_its_span() {
                    orch_api_admin::routes::admin::audit::tests::__api_run_a_file_is_named_by_its_span()
                }
            }
        }

        pub mod catalogue {

            pub use orch_api_admin::routes::admin::catalogue::mount;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[tokio::test]
                async fn a_running_build_without_a_catalogue_check_keeps_its_declared_localized_name()
                 {
                    orch_api_admin::routes::admin::catalogue::tests::__api_run_a_running_build_without_a_catalogue_check_keeps_its_declared_localized_name().await
                }
                #[cfg(test)]
                #[test]
                fn a_reported_version_has_no_borrowed_check_without_the_same_frozen_bytes() {
                    orch_api_admin::routes::admin::catalogue::tests::__api_run_a_reported_version_has_no_borrowed_check_without_the_same_frozen_bytes()
                }
                #[cfg(test)]
                #[test]
                fn a_capability_key_is_what_the_schema_allows() {
                    orch_api_admin::routes::admin::catalogue::tests::__api_run_a_capability_key_is_what_the_schema_allows()
                }
            }
        }

        pub mod fleet {

            pub use orch_api_admin::routes::admin::fleet::mount;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn a_span_gets_the_finest_step_that_gives_at_most_720_points() {
                    orch_api_admin::routes::admin::fleet::tests::__api_run_a_span_gets_the_finest_step_that_gives_at_most_720_points()
                }
                #[cfg(test)]
                #[test]
                fn a_step_that_is_asked_for_must_be_one_of_the_four_and_fit() {
                    orch_api_admin::routes::admin::fleet::tests::__api_run_a_step_that_is_asked_for_must_be_one_of_the_four_and_fit()
                }
            }
        }

        pub mod identity_providers {

            pub use orch_api_admin::routes::admin::identity_providers::mount;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn a_key_is_what_appears_in_a_callback_address() {
                    orch_api_admin::routes::admin::identity_providers::tests::__api_run_a_key_is_what_appears_in_a_callback_address()
                }
                #[cfg(test)]
                #[test]
                fn scopes_must_include_openid_and_are_kept_once() {
                    orch_api_admin::routes::admin::identity_providers::tests::__api_run_scopes_must_include_openid_and_are_kept_once()
                }
                #[cfg(test)]
                #[test]
                fn domains_are_lower_case_dotted_names_without_an_at_sign() {
                    orch_api_admin::routes::admin::identity_providers::tests::__api_run_domains_are_lower_case_dotted_names_without_an_at_sign()
                }
            }
        }

        pub mod orgs {

            pub use orch_api_admin::routes::admin::orgs::mount;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn a_change_of_limits_is_told_in_the_words_of_what_it_does() {
                    orch_api_admin::routes::admin::orgs::tests::__api_run_a_change_of_limits_is_told_in_the_words_of_what_it_does()
                }
                #[cfg(test)]
                #[test]
                fn a_cursor_of_orgs_keeps_a_name_with_any_characters() {
                    orch_api_admin::routes::admin::orgs::tests::__api_run_a_cursor_of_orgs_keeps_a_name_with_any_characters()
                }
            }
        }

        pub mod programs {

            pub use orch_api_admin::routes::admin::programs::mount;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn a_cursor_binds_the_list_search_program_and_filters() {
                    orch_api_admin::routes::admin::programs::tests::__api_run_a_cursor_binds_the_list_search_program_and_filters()
                }
                #[cfg(test)]
                #[test]
                fn missing_declarations_are_not_reported_and_checks_cannot_turn_a_capability_back_on()
                 {
                    orch_api_admin::routes::admin::programs::tests::__api_run_missing_declarations_are_not_reported_and_checks_cannot_turn_a_capability_back_on()
                }
            }
        }

        pub mod users {

            pub use orch_api_admin::routes::admin::users::mount;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn a_count_that_is_full_locks_until_its_oldest_needed_try_is_a_window_old() {
                    orch_api_admin::routes::admin::users::tests::__api_run_a_count_that_is_full_locks_until_its_oldest_needed_try_is_a_window_old()
                }
                #[cfg(test)]
                #[test]
                fn a_username_is_what_the_schema_allows() {
                    orch_api_admin::routes::admin::users::tests::__api_run_a_username_is_what_the_schema_allows()
                }
            }
        }
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn an_address_is_http_or_https_with_a_host_and_no_space() {
                orch_api_admin::routes::admin::tests::__api_run_an_address_is_http_or_https_with_a_host_and_no_space()
            }
        }

        pub mod defaults {

            pub use orch_api_admin::routes::admin::defaults::mount;
        }

        pub mod sessions {

            pub use orch_api_admin::routes::admin::sessions::mount;
        }

        pub mod tokens {

            pub use orch_api_admin::routes::admin::tokens::mount;
        }

        pub mod settings {

            pub use orch_api_admin::routes::admin::settings::mount;
        }

        pub mod semantic {

            pub use orch_api_admin::routes::admin::semantic::mount;
        }

        pub mod system {

            pub use orch_api_admin::routes::admin::system::install_secrets_dir;

            pub use orch_api_admin::routes::admin::system::mount;
        }
    }

    pub mod auth {

        pub use orch_api_admin::routes::auth::mount;

        pub mod oidc {

            pub use orch_api_admin::routes::auth::oidc::PROOF_LIFETIME;

            pub use orch_api_admin::routes::auth::oidc::PROOF_PREFIX;

            pub use orch_api_admin::routes::auth::oidc::PROOF_FRAGMENT;

            pub use orch_api_admin::routes::auth::oidc::PROOF_ERROR_FRAGMENT;

            pub use orch_api_admin::routes::auth::oidc::NOT_FRESH;

            pub use orch_api_admin::routes::auth::oidc::NO_SESSION;

            pub use orch_api_admin::routes::auth::oidc::LINK_ERROR_FRAGMENT;

            pub use orch_api_admin::routes::auth::oidc::BROWSER_COOKIE;

            pub use orch_api_admin::routes::auth::oidc::BROWSER_MARK_PREFIX;

            pub use orch_api_admin::routes::auth::oidc::install_kind;

            pub use orch_api_admin::routes::auth::oidc::mount;

            pub mod landing {
                #[cfg(test)]
                mod tests {
                    #[cfg(test)]
                    #[test]
                    fn a_long_base_is_cut_to_where_every_numbered_name_begins() {
                        orch_api_admin::routes::auth::oidc::landing::tests::__api_run_a_long_base_is_cut_to_where_every_numbered_name_begins()
                    }
                    #[cfg(test)]
                    #[test]
                    fn the_registration_mode_of_the_system_is_the_one_the_rules_read() {
                        orch_api_admin::routes::auth::oidc::landing::tests::__api_run_the_registration_mode_of_the_system_is_the_one_the_rules_read()
                    }
                }
            }
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn a_refusal_goes_where_the_sign_in_was_for() {
                    orch_api_admin::routes::auth::oidc::tests::__api_run_a_refusal_goes_where_the_sign_in_was_for()
                }
                #[cfg(test)]
                #[test]
                fn the_mark_of_a_browser_is_sent_back_by_a_navigation_from_the_provider_and_only_here()
                 {
                    orch_api_admin::routes::auth::oidc::tests::__api_run_the_mark_of_a_browser_is_sent_back_by_a_navigation_from_the_provider_and_only_here()
                }
                #[cfg(test)]
                #[test]
                fn a_proof_lasts_five_minutes() {
                    orch_api_admin::routes::auth::oidc::tests::__api_run_a_proof_lasts_five_minutes(
                    )
                }
            }
        }
        #[cfg(test)]
        pub(crate) mod token_activity {
            #[cfg(test)]
            #[allow(clippy::expect_used, reason = "a test fails by panicking")]
            mod tests {
                #[cfg(test)]
                #[test]
                fn history_cursors_are_bound_to_the_actual_key_and_org_slice() {
                    orch_api_admin::routes::auth::token_activity::tests::__api_run_history_cursors_are_bound_to_the_actual_key_and_org_slice()
                }
            }
        }

        pub mod tokens {

            pub use orch_api_admin::routes::auth::tokens::mount;

            pub use orch_api_admin::routes::auth::tokens::Asked;

            pub use orch_api_admin::routes::auth::tokens::asked;

            pub use orch_api_admin::routes::auth::tokens::scope_info;

            pub use orch_api_admin::routes::auth::tokens::within_lifetime;

            pub use orch_api_admin::routes::auth::tokens::replayed;

            pub use orch_api_admin::routes::auth::tokens::keep;
            #[cfg(test)]
            #[allow(clippy::unwrap_used, reason = "a boundary control fails by panicking")]
            mod tests {
                #[cfg(test)]
                #[test]
                fn omitted_modes_create_explicit_deny_all_and_explicit_legacy_fields_stay_distinct()
                {
                    orch_api_admin::routes::auth::tokens::tests::__api_run_omitted_modes_create_explicit_deny_all_and_explicit_legacy_fields_stay_distinct()
                }
                #[cfg(test)]
                #[test]
                fn issuance_rejects_unknown_future_selections_before_storage() {
                    orch_api_admin::routes::auth::tokens::tests::__api_run_issuance_rejects_unknown_future_selections_before_storage()
                }
                #[cfg(test)]
                #[test]
                fn key_issuance_requires_complete_current_admission_and_cannot_issue_a_legacy_escape()
                 {
                    orch_api_admin::routes::auth::tokens::tests::__api_run_key_issuance_requires_complete_current_admission_and_cannot_issue_a_legacy_escape()
                }
            }
        }

        pub mod me {

            pub use orch_api_admin::routes::auth::me::mount;
        }
    }

    pub mod personal {

        pub use orch_api_admin::routes::personal::mount;
        #[cfg(test)]
        pub(crate) mod viewed {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn only_the_served_complete_page_can_be_recorded() {
                    orch_api_admin::routes::personal::viewed::tests::__api_run_only_the_served_complete_page_can_be_recorded()
                }
                #[cfg(test)]
                #[test]
                fn overview_visits_require_the_served_complete_place() {
                    orch_api_admin::routes::personal::viewed::tests::__api_run_overview_visits_require_the_served_complete_place()
                }
                #[cfg(test)]
                #[test]
                fn a_documents_own_node_is_kept_inside_the_selected_document() {
                    orch_api_admin::routes::personal::viewed::tests::__api_run_a_documents_own_node_is_kept_inside_the_selected_document()
                }
            }
        }
    }

    pub mod settings {

        pub use orch_api_admin::routes::settings::install_master_key;

        pub use orch_api_admin::routes::settings::mount;
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn every_registry_definition_and_merge_word_has_the_same_api_contract()
            -> ::orch_api_base::middleware::requests::ApiResult<()> {
                orch_api_admin::routes::settings::tests::__api_run_every_registry_definition_and_merge_word_has_the_same_api_contract()
            }
            #[cfg(test)]
            #[test]
            fn unsupported_auto_remains_a_visible_declaration_without_joining_the_offered_enum()
            -> ::orch_api_base::middleware::requests::ApiResult<()> {
                orch_api_admin::routes::settings::tests::__api_run_unsupported_auto_remains_a_visible_declaration_without_joining_the_offered_enum()
            }
            #[cfg(test)]
            #[test]
            fn unavailable_approval_metadata_does_not_change_other_registry_schemas() {
                orch_api_admin::routes::settings::tests::__api_run_unavailable_approval_metadata_does_not_change_other_registry_schemas()
            }
            #[cfg(test)]
            #[test]
            fn zero_disables_these_features_without_losing_their_units() {
                orch_api_admin::routes::settings::tests::__api_run_zero_disables_these_features_without_losing_their_units()
            }
            #[cfg(test)]
            #[test]
            fn an_unbounded_person_notification_rate_uses_the_existing_no_limit_display() {
                orch_api_admin::routes::settings::tests::__api_run_an_unbounded_person_notification_rate_uses_the_existing_no_limit_display()
            }
        }
    }

    pub mod users {

        pub use orch_api_admin::routes::users::mount;
        #[cfg(test)]
        pub(crate) mod invitation_terms {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn the_public_hash_is_sha256_of_the_exact_resolved_utf8_text() {
                    orch_api_admin::routes::users::invitation_terms::tests::__api_run_the_public_hash_is_sha256_of_the_exact_resolved_utf8_text()
                }
            }
        }
    }

    pub mod access {

        pub use orch_api_collaboration::routes::access::mount;
        #[cfg(test)]
        pub(crate) mod page_operations {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[tokio::test]
                async fn refusal_keeps_opaque_trusted_or_untrusted_error_and_changes_no_page_audit_or_native_dto()
                 {
                    orch_api_collaboration::routes::access::page_operations::tests::__api_run_refusal_keeps_opaque_trusted_or_untrusted_error_and_changes_no_page_audit_or_native_dto().await
                }
                #[cfg(test)]
                #[tokio::test]
                async fn genuine_noop_never_calls_boundary_and_multirow_private_change_calls_only_once()
                 {
                    orch_api_collaboration::routes::access::page_operations::tests::__api_run_genuine_noop_never_calls_boundary_and_multirow_private_change_calls_only_once().await
                }
                #[cfg(test)]
                #[tokio::test]
                async fn an_empty_owners_object_cannot_relabel_a_whole_page_change_as_an_ownership_operation()
                 {
                    orch_api_collaboration::routes::access::page_operations::tests::__api_run_an_empty_owners_object_cannot_relabel_a_whole_page_change_as_an_ownership_operation().await
                }
            }
        }

        pub mod share_preview {

            pub use orch_api_collaboration::routes::access::share_preview::create;
        }

        pub mod previews {

            pub use orch_api_collaboration::routes::access::previews::page;

            pub use orch_api_collaboration::routes::access::previews::node_defaults;

            pub use orch_api_collaboration::routes::access::previews::admin_defaults;
        }

        pub mod dependencies {}

        pub mod requests {

            pub use orch_api_collaboration::routes::access::requests::create;

            pub use orch_api_collaboration::routes::access::requests::get;

            pub use orch_api_base::routes::access::requests::Shown;

            pub use orch_api_collaboration::routes::access::requests::views_of;

            pub use orch_api_collaboration::routes::access::requests::sent_to;

            pub use orch_api_collaboration::routes::access::requests::decide_row;
        }

        pub mod page {

            pub use orch_api_base::routes::access::page::same;

            pub use orch_api_base::routes::access::page::Names;

            pub use orch_api_collaboration::routes::access::page::permission_page;

            pub use orch_api_collaboration::routes::access::page::entries_named;

            pub use orch_api_collaboration::routes::access::page::entries_named_at;
        }

        pub mod candidates {}

        pub mod thread_environment {

            pub use orch_api_collaboration::routes::access::thread_environment::show;
        }

        pub mod readers {

            pub use orch_api_collaboration::routes::access::readers::effective;

            pub use orch_api_collaboration::routes::access::readers::people;

            pub use orch_api_collaboration::routes::access::readers::history;
        }

        pub mod subject {

            pub use orch_api_collaboration::routes::access::subject::Named;

            pub use orch_api_collaboration::routes::access::subject::named;

            pub use orch_api_collaboration::routes::access::subject::kind_of_word;

            pub use orch_api_collaboration::routes::access::subject::of;

            pub use orch_api_collaboration::routes::access::subject::policy_of;

            pub use orch_api_collaboration::routes::access::subject::policy_at_place;

            pub use orch_api_collaboration::routes::access::subject::resource_policy;

            pub use orch_api_collaboration::routes::access::subject::require_resource_reader;

            pub use orch_api_collaboration::routes::access::subject::require_view;

            pub use orch_api_collaboration::routes::access::subject::require_page;

            pub use orch_api_collaboration::routes::access::subject::require_manage;
        }
    }

    pub mod nodes {

        pub use orch_api_collaboration::routes::nodes::NAME_MAX_CHARS;

        pub use orch_api_collaboration::routes::nodes::DESCRIPTION_MAX_CHARS;

        pub use orch_api_collaboration::routes::nodes::GENERAL_PROJECT;

        pub use orch_api_base::routes::nodes::NEW_TOPIC;

        pub use orch_api_collaboration::routes::nodes::mount;

        pub use orch_api_collaboration::routes::nodes::invalid;

        pub use orch_api_collaboration::routes::nodes::clean_name;

        pub use orch_api_collaboration::routes::nodes::clean_description;

        pub use orch_api_collaboration::routes::nodes::threads_are_not_served_here;

        pub use orch_api_collaboration::routes::nodes::policy_at;

        pub use orch_api_base::routes::nodes::Placed;

        pub use orch_api_collaboration::routes::nodes::record;

        pub use orch_api_collaboration::routes::nodes::record_about;

        pub use orch_api_collaboration::routes::nodes::placed;

        pub use orch_api_collaboration::routes::nodes::actor_name;

        pub use orch_api_collaboration::routes::nodes::node_id;

        pub use orch_api_collaboration::routes::nodes::node_participating;

        pub use orch_api_collaboration::routes::nodes::node_archived;

        pub use orch_api_collaboration::routes::nodes::node_revision;

        pub use orch_api_collaboration::routes::nodes::shown;

        pub use orch_api_collaboration::routes::nodes::before_name;

        pub use orch_api_collaboration::routes::nodes::stale;

        pub use orch_api_collaboration::routes::nodes::permission_to_make;

        pub use orch_api_collaboration::routes::nodes::require_manage;
        #[cfg(test)]
        pub(crate) mod tree_work {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn a_readers_approval_is_not_replaced_by_someone_elses_question() {
                    orch_api_collaboration::routes::nodes::tree_work::tests::__api_run_a_readers_approval_is_not_replaced_by_someone_elses_question()
                }
                #[cfg(test)]
                #[test]
                fn only_a_known_current_binding_refusal_names_its_setup_cause() {
                    orch_api_collaboration::routes::nodes::tree_work::tests::__api_run_only_a_known_current_binding_refusal_names_its_setup_cause()
                }
            }
        }

        pub mod removal {

            pub use orch_api_collaboration::routes::nodes::removal::mount;
        }

        pub mod archive_plan {

            pub use orch_api_collaboration::routes::nodes::archive_plan::mount;
        }

        pub mod tree {

            pub use orch_api_collaboration::routes::nodes::tree::mount;
        }

        pub mod make {

            pub use orch_api_collaboration::routes::nodes::make::ROOT_SLUG_MIN_CHARS;

            pub use orch_api_collaboration::routes::nodes::make::SLUG_MIN_CHARS;

            pub use orch_api_collaboration::routes::nodes::make::SLUG_MAX_CHARS;

            pub use orch_api_collaboration::routes::nodes::make::mount;
        }

        pub mod activity {

            pub use orch_api_collaboration::routes::nodes::activity::mount;
        }

        pub mod view {

            pub use orch_api_base::routes::nodes::view::Built;

            pub use orch_api_collaboration::routes::nodes::view::kind_of;

            pub use orch_api_collaboration::routes::nodes::view::every_permission;

            pub use orch_api_collaboration::routes::nodes::view::build;

            pub use orch_api_collaboration::routes::nodes::view::visible;

            pub use orch_api_collaboration::routes::nodes::view::one;

            pub use orch_api_collaboration::routes::nodes::view::ways;

            pub use orch_api_collaboration::routes::nodes::view::ways_with_admin;

            pub use orch_api_collaboration::routes::nodes::view::refs;

            pub use orch_api_collaboration::routes::nodes::view::qualified_refs;

            pub use orch_api_collaboration::routes::nodes::view::parent_of;
        }

        pub mod moves {

            pub use orch_api_collaboration::routes::nodes::moves::mount;

            pub mod thread {

                pub use orch_api_collaboration::routes::nodes::moves::thread::mount;
            }
        }
    }

    pub mod notifications {

        pub use orch_api_collaboration::routes::notifications::install_push_public_key;

        pub use orch_api_collaboration::routes::notifications::install_notifiers;

        pub use orch_api_collaboration::routes::notifications::mount;

        pub mod views {

            pub use orch_api_collaboration::routes::notifications::views::needs_view;

            pub use orch_api_collaboration::routes::notifications::views::kind_of;

            pub use orch_api_collaboration::routes::notifications::views::RouteFacts;

            pub use orch_api_collaboration::routes::notifications::views::health_view;

            pub use orch_api_collaboration::routes::notifications::views::failure_view;

            pub use orch_api_collaboration::routes::notifications::views::route_channel_of;

            pub use orch_api_collaboration::routes::notifications::views::route_channel_kept;

            pub use orch_api_collaboration::routes::notifications::views::route_view;

            pub use orch_api_collaboration::routes::notifications::views::templates_view;

            pub use orch_api_collaboration::routes::notifications::views::overrides_of;

            pub use orch_api_collaboration::routes::notifications::views::owner_view;

            pub use orch_api_collaboration::routes::notifications::views::ChannelFacts;

            pub use orch_api_collaboration::routes::notifications::views::channel_view;

            pub use orch_api_collaboration::routes::notifications::views::level_of;

            pub use orch_api_collaboration::routes::notifications::views::level_word;

            pub use orch_api_collaboration::routes::notifications::views::destination_view;

            pub use orch_api_collaboration::routes::notifications::views::notifier_view;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn a_member_sees_a_channel_without_its_fields_its_secrets_its_failures_and_its_count()
                 {
                    orch_api_collaboration::routes::notifications::views::tests::__api_run_a_member_sees_a_channel_without_its_fields_its_secrets_its_failures_and_its_count()
                }
                #[cfg(test)]
                #[test]
                fn a_route_nobody_changed_is_off_for_everything_and_at_once() {
                    orch_api_collaboration::routes::notifications::views::tests::__api_run_a_route_nobody_changed_is_off_for_everything_and_at_once()
                }
                #[cfg(test)]
                #[test]
                fn the_texts_of_a_channel_are_read_back_as_they_were_kept() {
                    orch_api_collaboration::routes::notifications::views::tests::__api_run_the_texts_of_a_channel_are_read_back_as_they_were_kept()
                }
            }
        }

        pub mod channels {

            pub use orch_api_collaboration::routes::notifications::channels::create_system;

            pub use orch_api_collaboration::routes::notifications::channels::create_org;

            pub use orch_api_collaboration::routes::notifications::channels::create_own;

            pub use orch_api_collaboration::routes::notifications::channels::delete;

            pub use orch_api_collaboration::routes::notifications::channels::test;
        }

        pub mod destinations {

            pub use orch_api_collaboration::routes::notifications::destinations::list;

            pub use orch_api_collaboration::routes::notifications::destinations::create;

            pub use orch_api_collaboration::routes::notifications::destinations::update;

            pub use orch_api_collaboration::routes::notifications::destinations::delete;

            pub use orch_api_collaboration::routes::notifications::destinations::test;
        }

        pub mod inbox {

            pub use orch_api_base::routes::notifications::inbox::Summary;

            pub use orch_api_collaboration::routes::notifications::inbox::summary;

            pub use orch_api_collaboration::routes::notifications::inbox::list;

            pub use orch_api_collaboration::routes::notifications::inbox::summary_get;

            pub use orch_api_collaboration::routes::notifications::inbox::outside_status;

            pub use orch_api_collaboration::routes::notifications::inbox::read;

            pub use orch_api_collaboration::routes::notifications::inbox::prefs_get;

            pub use orch_api_collaboration::routes::notifications::inbox::prefs_set;

            pub use orch_api_collaboration::routes::notifications::inbox::push_create;

            pub use orch_api_collaboration::routes::notifications::inbox::push_delete;

            pub use orch_api_collaboration::routes::notifications::inbox::push_list;

            pub use orch_api_collaboration::routes::notifications::inbox::push_remove;

            pub use orch_api_collaboration::routes::notifications::inbox::notifier_list;
        }

        pub mod grouping {

            pub use orch_api_collaboration::routes::notifications::grouping::list_by_place;

            pub use orch_api_collaboration::routes::notifications::grouping::read_group;

            pub use orch_api_collaboration::routes::notifications::grouping::undo_group;
        }

        pub mod routing {

            pub use orch_api_collaboration::routes::notifications::routing::set;

            pub use orch_api_collaboration::routes::notifications::routing::list;

            pub use orch_api_collaboration::routes::notifications::routing::remove;

            pub use orch_api_collaboration::routes::notifications::routing::link_create;

            pub use orch_api_collaboration::routes::notifications::routing::delete;
        }

        pub mod exceptions {}
    }

    pub mod search {

        pub use orch_api_collaboration::routes::search::NAME_BATCHES_MAX;

        pub use orch_api_collaboration::routes::search::mount;
        #[cfg(test)]
        mod content {
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn excerpts_and_highlights_use_characters_for_any_script_and_lowercase_expansions()
                {
                    orch_api_collaboration::routes::search::content::tests::__api_run_excerpts_and_highlights_use_characters_for_any_script_and_lowercase_expansions()
                }
            }
        }
    }

    pub mod transfer {

        pub use orch_api_collaboration::routes::transfer::mount;
        #[cfg(test)]
        mod tests {
            #[cfg(test)]
            #[test]
            fn transfer_kind_refusals_name_the_kinds_the_request_answers_for() {
                orch_api_collaboration::routes::transfer::tests::__api_run_transfer_kind_refusals_name_the_kinds_the_request_answers_for()
            }
        }
    }

    pub mod usage {

        pub use orch_api_interactive::routes::usage::mount;

        pub mod csv {

            pub use orch_api_interactive::routes::usage::csv::CONTENT_TYPE;

            pub use orch_api_interactive::routes::usage::csv::cell;

            pub use orch_api_interactive::routes::usage::csv::line;

            pub use orch_api_interactive::routes::usage::csv::response;

            pub use orch_api_interactive::routes::usage::csv::json_response;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn native_text_cannot_become_a_spreadsheet_formula_even_behind_controls() {
                    orch_api_interactive::routes::usage::csv::tests::__api_run_native_text_cannot_become_a_spreadsheet_formula_even_behind_controls()
                }
                #[cfg(test)]
                #[test]
                fn statistics_keep_exact_numeric_bytes_and_ordinary_native_words() {
                    orch_api_interactive::routes::usage::csv::tests::__api_run_statistics_keep_exact_numeric_bytes_and_ordinary_native_words()
                }
                #[cfg(test)]
                #[test]
                fn a_cell_with_a_comma_a_quote_or_a_line_break_is_quoted_and_its_quotes_are_doubled()
                 {
                    orch_api_interactive::routes::usage::csv::tests::__api_run_a_cell_with_a_comma_a_quote_or_a_line_break_is_quoted_and_its_quotes_are_doubled()
                }
            }
        }

        pub mod figures {

            pub use orch_api_interactive::routes::usage::figures::figures;

            pub use orch_api_interactive::routes::usage::figures::default_bucket;

            pub use orch_api_interactive::routes::usage::figures::get;

            pub use orch_api_interactive::routes::usage::figures::health;
            #[cfg(test)]
            mod tests {
                #[cfg(test)]
                #[test]
                fn degradation_requires_ten_requests_and_twenty_percent_errors_or_more_than_triple_latency()
                 {
                    orch_api_interactive::routes::usage::figures::tests::__api_run_degradation_requires_ten_requests_and_twenty_percent_errors_or_more_than_triple_latency()
                }
                #[cfg(test)]
                #[test]
                fn a_span_is_cut_by_the_hour_up_to_two_days_by_the_day_to_92_by_the_week_to_two_years_and_then_by_the_month()
                 {
                    orch_api_interactive::routes::usage::figures::tests::__api_run_a_span_is_cut_by_the_hour_up_to_two_days_by_the_day_to_92_by_the_week_to_two_years_and_then_by_the_month()
                }
                #[cfg(test)]
                #[test]
                fn every_bucket_of_a_span_has_a_point_and_a_week_begins_on_the_monday_before_the_span()
                 {
                    orch_api_interactive::routes::usage::figures::tests::__api_run_every_bucket_of_a_span_has_a_point_and_a_week_begins_on_the_monday_before_the_span()
                }
                #[cfg(test)]
                #[test]
                fn credits_are_written_in_their_shortest_exact_form() {
                    orch_api_interactive::routes::usage::figures::tests::__api_run_credits_are_written_in_their_shortest_exact_form()
                }
            }
        }

        pub mod names {

            pub use orch_api_base::routes::usage::names::Names;

            pub use orch_api_interactive::routes::usage::names::rate_of;
        }

        pub mod rates {

            pub use orch_api_interactive::routes::usage::rates::table;

            pub use orch_api_interactive::routes::usage::rates::catalogue;

            pub use orch_api_interactive::routes::usage::rates::set;
        }

        pub mod caps {

            pub use orch_api_interactive::routes::usage::caps::list;

            pub use orch_api_interactive::routes::usage::caps::limits;

            pub use orch_api_interactive::routes::usage::caps::mine;

            pub use orch_api_interactive::routes::usage::caps::waiting;

            pub use orch_api_interactive::routes::usage::caps::create;

            pub use orch_api_interactive::routes::usage::caps::update;

            pub use orch_api_interactive::routes::usage::caps::delete;
        }

        pub mod events {

            pub use orch_api_interactive::routes::usage::events::NEXT_CURSOR_HEADER;

            pub use orch_api_interactive::routes::usage::events::list;
        }

        pub mod scope {

            pub use orch_api_base::routes::usage::scope::Scope;

            pub use orch_api_interactive::routes::usage::scope::parse;

            pub use orch_api_interactive::routes::usage::scope::parse_day;

            pub use orch_api_interactive::routes::usage::scope::wants_csv;

            pub use orch_api_base::routes::usage::scope::Opened;

            pub use orch_api_base::routes::usage::scope::Wanted;

            pub use orch_api_interactive::routes::usage::scope::open;

            pub use orch_api_interactive::routes::usage::scope::gives;

            pub use orch_api_interactive::routes::usage::scope::require_dimension;

            pub use orch_api_interactive::routes::usage::scope::filter;

            pub use orch_api_interactive::routes::usage::scope::all_record_places;

            pub use orch_api_interactive::routes::usage::scope::visible_records;
        }
    }

    pub mod imports {

        pub use orch_api_documents::routes::imports::mount;
    }

    pub mod ports {

        pub use orch_api_interactive::routes::ports::mount;

        pub use orch_api_interactive::routes::ports::exposures;

        pub use orch_api_interactive::routes::ports::address;

        pub use orch_api_interactive::routes::ports::target;

        pub use orch_api_interactive::routes::ports::check_program;
    }

    pub mod port_entry {

        pub use orch_api_interactive::routes::port_entry::PORT_SUBPROTOCOL;

        pub use orch_api_interactive::routes::port_entry::END_OF_DIRECTION;

        pub use orch_api_interactive::routes::port_entry::install_port_signer;

        pub use orch_api_interactive::routes::port_entry::mount;
    }

    pub mod downloads {

        pub use orch_api_documents::routes::downloads::mount;

        pub use orch_api_documents::routes::downloads::payload_dir;
    }

    pub mod admin_messages {

        pub use orch_api_admin::routes::admin_messages::mount;
    }

    pub mod loss {

        pub use orch_api_environments::routes::loss::mount;
    }

    pub mod displays {

        pub use orch_api_interactive::routes::displays::mount;
    }

    pub mod links {

        pub use orch_api_threads::routes::links::mount;
    }

    pub mod processes {

        pub use orch_api_environments::routes::processes::mount;

        pub use orch_api_environments::routes::processes::taken_at;
    }

    pub mod port_requests {

        pub use orch_api_interactive::routes::port_requests::mount;
    }

    pub mod share {

        pub use orch_api_threads::routes::share::mount;
    }

    pub mod gateways {

        pub use orch_api_interactive::routes::gateways::mount;
    }

    pub mod directory {

        pub use orch_api_admin::routes::directory::mount;
    }

    pub mod fleet {

        pub use orch_api_environments::routes::fleet::mount;
    }

    pub mod everything {

        pub use orch_api_collaboration::routes::everything::mount;
    }

    pub mod exports {

        pub use orch_api_documents::routes::exports::mount;

        pub use orch_api_base::routes::exports::ApiForms;
    }

    pub mod browsers {

        pub use orch_api_interactive::routes::browsers::mount;
    }

    pub mod document_tree {

        pub use orch_api_documents::routes::document_tree::mount;
    }

    pub mod topics {

        pub use orch_api_threads::routes::topics::mount;

        pub use orch_api_base::routes::topics::mount_announcements;
    }

    pub mod chats {

        pub use orch_api_collaboration::routes::chats::mount;

        pub use orch_api_collaboration::routes::chats::failed;

        pub use orch_api_collaboration::routes::chats::chat_view;

        pub use orch_api_collaboration::routes::chats::messages_view;
    }

    pub mod trust {

        pub use orch_api_resources::routes::trust::mount;
    }

    pub mod terminals {

        pub use orch_api_interactive::routes::terminals::mount;
    }

    pub mod profiles {

        pub use orch_api_admin::routes::profiles::mount;
    }

    pub mod capabilities {

        pub use orch_api_resources::routes::capabilities::mount;
    }

    pub mod approvals {

        pub use orch_api_threads::routes::approvals::mount;
    }

    pub mod duplicates {

        pub use orch_api_resources::routes::duplicates::mount;
    }

    pub mod storage {

        pub use orch_api_documents::routes::storage::mount;
    }

    pub mod meta {

        pub use orch_api_interactive::routes::meta::mount;

        pub use orch_api_interactive::routes::meta::health_router;
    }

    pub mod first_steps {

        pub use orch_api_admin::routes::first_steps::mount;
    }
    pub fn mount(registry: &mut orch_api_base::registry::Registry) {
        orch_api_base::route_assembly::initialize();
        orch_api_base::routes::mount(registry);
    }
}

pub mod uploads {

    pub use orch_api_base::uploads::SESSION_LIFE;

    pub use orch_api_base::uploads::NAME_MAX_CHARS;

    pub use orch_api_base::uploads::chunk_bytes;

    pub use orch_api_base::uploads::max_blob_bytes;

    pub use orch_api_base::uploads::clean_name;

    pub use orch_api_base::uploads::stored_badly;

    pub use orch_api_base::uploads::begin;

    pub use orch_api_base::uploads::cancel;

    pub use orch_api_base::uploads::put_piece;

    pub use orch_api_base::uploads::complete;

    pub use orch_api_base::uploads::register_whole;
}

pub mod testing {

    pub use orch_api_base::testing::recover_ask_turns;

    pub use orch_api_base::testing::machine_request_handler;

    pub use orch_api_base::testing::advance_component_updates;

    pub use orch_api_base::testing::authorize_machine;

    pub use orch_api_base::testing::mirror_native_result;

    pub use orch_api_base::testing::captures_native_creation;

    pub use orch_api_base::testing::observe_native_creation;

    pub use orch_api_base::testing::TestApp;

    pub use orch_api_base::testing::Reply;

    pub use orch_api_base::testing::RawReply;

    pub use orch_api_base::testing::current_identity_query_pattern;

    pub use orch_api_base::testing::Call;

    pub use orch_api_base::testing::Machine;

    pub use orch_api_base::testing::Seen;

    pub use orch_api_base::testing::Played;

    pub use orch_api_base::testing::Boundaries;

    pub use orch_api_base::testing::follow;
}

pub mod recordings {

    pub use orch_api_base::recordings::KeptOutput;

    pub use orch_api_base::recordings::install;
}

pub mod component_updates {

    pub use orch_api_base::component_updates::views;

    pub use orch_api_base::component_updates::newer;
}

pub mod thread_access {

    pub use orch_api_base::thread_access::AuthzAccess;
}

pub mod server {

    pub use orch_api_base::server::router;

    pub use orch_api_base::server::serve_plain;

    pub use orch_api_base::server::Peer;

    pub use orch_api_base::server::NotThatService;

    pub use orch_api_base::server::browser_tls_config;

    pub use orch_api_base::server::internal_tls_config;

    pub use orch_api_base::server::browser_tls_config_live;

    pub use orch_api_base::server::internal_tls_config_live;

    pub use orch_api_base::server::serve_tls;

    pub use orch_api_base::server::internal_router;
}

pub mod openapi {

    pub use orch_api_base::openapi::document;

    pub use orch_api_base::openapi::served;
}

pub mod thread_read_tools {

    pub use orch_api_base::thread_read_tools::install;
}

pub mod state {

    pub use orch_api_base::state::AppState;

    pub use orch_api_base::state::App;
}

pub mod machine_requests {

    pub use orch_api_base::machine_requests::Handler;

    pub use orch_api_base::machine_requests::install;

    pub use orch_api_base::machine_requests::import_from_machine;
}

pub mod harness_checks {

    pub use orch_api_base::harness_checks::check_waiting;

    pub use orch_api_base::harness_checks::check;

    pub use orch_api_base::harness_checks::claim;

    pub use orch_api_base::harness_checks::claim_in;

    pub use orch_api_base::harness_checks::claim_in_before;

    pub use orch_api_base::harness_checks::finish;

    pub use orch_api_base::harness_checks::kept;
}

pub mod thread_markers {}
fn initialize_route_families() {
    orch_api_threads::install_interfaces();
    orch_api_documents::install_interfaces();
    orch_api_resources::install_interfaces();
    orch_api_environments::install_interfaces();
    orch_api_admin::install_interfaces();
    orch_api_collaboration::install_interfaces();
    orch_api_interactive::install_interfaces();
}
inventory::submit! { orch_api_base::route_assembly::Assembly { initialize: initialize_route_families, mount: routes::mount } }

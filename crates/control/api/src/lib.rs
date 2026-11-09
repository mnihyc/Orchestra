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
pub use orch_link::test_observers;

pub mod archives {
    pub use orch_api_base::archives::*;
    #[cfg(test)]
    pub mod tests {
        pub use orch_api_base::archives::tests::*;
        #[tokio::test(flavor = "multi_thread")]
        async fn one_entry_of_a_large_zip_is_read_by_asking_the_store_for_a_small_part_of_it() {
            orch_api_base::archives::tests::__api_run_one_entry_of_a_large_zip_is_read_by_asking_the_store_for_a_small_part_of_it().await
        }
        #[tokio::test(flavor = "multi_thread")]
        async fn a_tar_is_found_in_every_compression_by_its_first_bytes_and_not_by_its_name() {
            orch_api_base::archives::tests::__api_run_a_tar_is_found_in_every_compression_by_its_first_bytes_and_not_by_its_name().await
        }
        #[tokio::test(flavor = "multi_thread")]
        async fn a_file_that_is_no_archive_is_refused_and_so_is_a_zip_that_is_cut_short() {
            orch_api_base::archives::tests::__api_run_a_file_that_is_no_archive_is_refused_and_so_is_a_zip_that_is_cut_short().await
        }
        #[tokio::test(flavor = "multi_thread")]
        async fn an_entry_is_read_up_to_the_limit_and_a_name_that_is_not_a_file_is_not_read() {
            orch_api_base::archives::tests::__api_run_an_entry_is_read_up_to_the_limit_and_a_name_that_is_not_a_file_is_not_read().await
        }
        #[tokio::test(flavor = "multi_thread")]
        async fn a_tar_that_is_read_for_longer_than_may_be_says_so() {
            orch_api_base::archives::tests::__api_run_a_tar_that_is_read_for_longer_than_may_be_says_so().await
        }
        #[test]
        fn a_zip_that_claims_more_entries_than_are_looked_into_is_known_by_its_end_record() {
            orch_api_base::archives::tests::__api_run_a_zip_that_claims_more_entries_than_are_looked_into_is_known_by_its_end_record()
        }
        #[tokio::test(flavor = "multi_thread")]
        async fn a_zip_with_a_password_is_listed_and_none_of_its_entries_is_opened() {
            orch_api_base::archives::tests::__api_run_a_zip_with_a_password_is_listed_and_none_of_its_entries_is_opened().await
        }
    }
}

pub(crate) mod ask {
    pub use orch_api_base::ask::*;

    pub mod addresses {
        pub use orch_api_base::ask::addresses::*;
        #[cfg(test)]
        #[allow(
            clippy::unwrap_used,
            clippy::expect_used,
            reason = "a deciding case fails by panicking"
        )]
        pub mod tests {
            pub use orch_api_base::ask::addresses::tests::*;
            #[test]
            fn links_accept_only_the_current_and_declared_normalized_origins() {
                orch_api_base::ask::addresses::tests::__api_run_links_accept_only_the_current_and_declared_normalized_origins()
            }
            #[test]
            fn links_reject_foreign_credentials_ports_schemes_controls_and_backslashes() {
                orch_api_base::ask::addresses::tests::__api_run_links_reject_foreign_credentials_ports_schemes_controls_and_backslashes()
            }
        }
    }

    pub mod search {
        pub use orch_api_base::ask::search::*;
        #[cfg(test)]
        #[allow(
            clippy::unwrap_used,
            clippy::expect_used,
            reason = "a deciding case fails by panicking"
        )]
        pub mod tests {
            pub use orch_api_base::ask::search::tests::*;
            #[test]
            fn ranked_search_keeps_native_sources_and_requires_its_terminal_cursor() {
                orch_api_base::ask::search::tests::__api_run_ranked_search_keeps_native_sources_and_requires_its_terminal_cursor()
            }
        }
    }

    pub mod tools {
        pub use orch_api_base::ask::tools::*;

        pub mod documentation {
            pub use orch_api_base::ask::tools::documentation::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_base::ask::tools::documentation::tests::*;
                #[test]
                fn canonical_headings_follow_front_matter_and_ignore_fenced_code()
                -> impl ::std::process::Termination {
                    orch_api_base::ask::tools::documentation::tests::__api_run_canonical_headings_follow_front_matter_and_ignore_fenced_code()
                }
                #[test]
                fn public_documentation_returns_both_languages_and_rejects_extra_arguments()
                -> impl ::std::process::Termination {
                    orch_api_base::ask::tools::documentation::tests::__api_run_public_documentation_returns_both_languages_and_rejects_extra_arguments()
                }
            }
        }

        pub mod parameters {
            pub use orch_api_base::ask::tools::parameters::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_base::ask::tools::parameters::tests::*;
                #[test]
                fn list_filters_retain_the_native_query_and_cannot_change_the_list_kind()
                -> impl ::std::process::Termination {
                    orch_api_base::ask::tools::parameters::tests::__api_run_list_filters_retain_the_native_query_and_cannot_change_the_list_kind()
                }
                #[test]
                fn document_history_keeps_native_cursor_and_replays_its_exclusive_lower_bound()
                -> impl ::std::process::Termination {
                    orch_api_base::ask::tools::parameters::tests::__api_run_document_history_keeps_native_cursor_and_replays_its_exclusive_lower_bound()
                }
                #[test]
                fn thread_and_chat_history_keep_their_native_sequence_windows()
                -> impl ::std::process::Termination {
                    orch_api_base::ask::tools::parameters::tests::__api_run_thread_and_chat_history_keep_their_native_sequence_windows()
                }
            }
        }

        pub mod references {
            pub use orch_api_base::ask::tools::references::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_base::ask::tools::references::tests::*;
                #[test]
                fn typed_resources_and_public_addresses_keep_real_typed_identities()
                -> impl ::std::process::Termination {
                    orch_api_base::ask::tools::references::tests::__api_run_typed_resources_and_public_addresses_keep_real_typed_identities()
                }
                #[test]
                fn public_parts_use_the_same_query_and_fragment_priority_as_reference_chips()
                -> impl ::std::process::Termination {
                    orch_api_base::ask::tools::references::tests::__api_run_public_parts_use_the_same_query_and_fragment_priority_as_reference_chips()
                }
            }
        }

        pub mod scope {
            pub use orch_api_base::ask::tools::scope::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_base::ask::tools::scope::tests::*;
                #[test]
                fn optional_context_cannot_promote_an_unrelated_required_resource()
                -> impl ::std::process::Termination {
                    orch_api_base::ask::tools::scope::tests::__api_run_optional_context_cannot_promote_an_unrelated_required_resource()
                }
            }
        }
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::ask::tools::tests::*;
            #[test]
            fn canonical_sources_require_checked_native_identities_and_preserve_redaction() {
                orch_api_base::ask::tools::tests::__api_run_canonical_sources_require_checked_native_identities_and_preserve_redaction()
            }
            #[test]
            fn repeated_and_json_query_fields_keep_the_native_form_encoding()
            -> impl ::std::process::Termination {
                orch_api_base::ask::tools::tests::__api_run_repeated_and_json_query_fields_keep_the_native_form_encoding()
            }
            #[test]
            fn every_reference_reader_resolves_to_a_registered_read_operation()
            -> impl ::std::process::Termination {
                orch_api_base::ask::tools::tests::__api_run_every_reference_reader_resolves_to_a_registered_read_operation()
            }
        }
    }
}

pub mod attachments {
    pub use orch_api_base::attachments::*;
    #[cfg(test)]
    pub mod tests {
        pub use orch_api_base::attachments::tests::*;
        #[test]
        fn a_claimed_type_is_kept_only_when_it_is_a_plain_type_and_subtype() {
            orch_api_base::attachments::tests::__api_run_a_claimed_type_is_kept_only_when_it_is_a_plain_type_and_subtype()
        }
        #[test]
        fn only_the_kinds_a_place_shows_are_shown_and_never_active_content() {
            orch_api_base::attachments::tests::__api_run_only_the_kinds_a_place_shows_are_shown_and_never_active_content()
        }
    }
}

pub mod backplane {
    pub use orch_api_base::backplane::*;

    pub mod tunnel_routing {
        pub use orch_api_base::backplane::tunnel_routing::*;
        #[cfg(test)]
        pub mod placement_tests {
            pub use orch_api_base::backplane::tunnel_routing::placement_tests::*;
            #[test]
            fn both_sides_choose_the_lowest_sum_and_ties_prefer_the_executor() {
                orch_api_base::backplane::tunnel_routing::placement_tests::__api_run_both_sides_choose_the_lowest_sum_and_ties_prefer_the_executor()
            }
            #[test]
            fn a_missing_measurement_is_not_a_zero_and_an_operator_pin_is_honoured() {
                orch_api_base::backplane::tunnel_routing::placement_tests::__api_run_a_missing_measurement_is_not_a_zero_and_an_operator_pin_is_honoured()
            }
            #[test]
            fn huge_scores_do_not_overflow_and_missing_candidates_never_win() {
                orch_api_base::backplane::tunnel_routing::placement_tests::__api_run_huge_scores_do_not_overflow_and_missing_candidates_never_win()
            }
        }
    }
    // Replacement enrollment and certificate handover through session authorization.
    #[cfg(test)]
    pub mod component_update_tests {
        pub use orch_api_base::backplane::component_update_tests::*;

        pub mod scopes {
            pub use orch_api_base::backplane::component_update_tests::scopes::*;
            #[tokio::test]
            async fn update_needs_exact_scope_and_returns_native_progress_only_under_its_own_read()
            {
                orch_api_base::backplane::component_update_tests::scopes::__api_run_update_needs_exact_scope_and_returns_native_progress_only_under_its_own_read().await
            }
            #[tokio::test]
            async fn actual_drain_loop_rechecks_original_scope_raw_expiry_revocation_and_physical_source()
             {
                orch_api_base::backplane::component_update_tests::scopes::__api_run_actual_drain_loop_rechecks_original_scope_raw_expiry_revocation_and_physical_source().await
            }
            #[tokio::test]
            async fn a_later_controller_does_not_replace_a_closed_original_update_issuer() {
                orch_api_base::backplane::component_update_tests::scopes::__api_run_a_later_controller_does_not_replace_a_closed_original_update_issuer().await
            }
            #[tokio::test]
            async fn retained_native_update_command_cannot_borrow_a_replacement_request_or_another_body()
             {
                orch_api_base::backplane::component_update_tests::scopes::__api_run_retained_native_update_command_cannot_borrow_a_replacement_request_or_another_body().await
            }
            #[tokio::test]
            async fn explicit_update_rechecks_at_its_actual_physical_and_first_request_insert_waits()
             {
                orch_api_base::backplane::component_update_tests::scopes::__api_run_explicit_update_rechecks_at_its_actual_physical_and_first_request_insert_waits().await
            }
            #[tokio::test]
            async fn completed_update_choice_keeps_one_request_when_response_read_closes_at_activity()
             {
                orch_api_base::backplane::component_update_tests::scopes::__api_run_completed_update_choice_keeps_one_request_when_response_read_closes_at_activity().await
            }
            #[tokio::test]
            async fn minimum_version_handshake_never_lends_the_automatic_path_to_a_closed_explicit_origin()
             {
                orch_api_base::backplane::component_update_tests::scopes::__api_run_minimum_version_handshake_never_lends_the_automatic_path_to_a_closed_explicit_origin().await
            }
        }
        #[tokio::test]
        async fn a_minimum_version_node_gets_one_audited_idle_grant_until_it_reconnects_on_the_recommendation()
         {
            orch_api_base::backplane::component_update_tests::__api_run_a_minimum_version_node_gets_one_audited_idle_grant_until_it_reconnects_on_the_recommendation().await
        }
    }
    #[cfg(test)]
    pub mod reenrollment_tests {
        pub use orch_api_base::backplane::reenrollment_tests::*;
        #[tokio::test]
        async fn a_revoked_machine_enrolls_again_without_losing_its_name_owner_or_registered_environment()
         {
            orch_api_base::backplane::reenrollment_tests::__api_run_a_revoked_machine_enrolls_again_without_losing_its_name_owner_or_registered_environment().await
        }
        #[tokio::test]
        async fn rejected_certificates_release_their_version_fence_before_authorization_answers() {
            orch_api_base::backplane::reenrollment_tests::__api_run_rejected_certificates_release_their_version_fence_before_authorization_answers().await
        }
        #[tokio::test]
        async fn an_active_old_certificate_is_retired_when_the_replacement_first_opens_a_session() {
            orch_api_base::backplane::reenrollment_tests::__api_run_an_active_old_certificate_is_retired_when_the_replacement_first_opens_a_session().await
        }
        #[tokio::test]
        async fn an_old_identity_cannot_renew_over_the_certificate_waiting_from_reenrollment() {
            orch_api_base::backplane::reenrollment_tests::__api_run_an_old_identity_cannot_renew_over_the_certificate_waiting_from_reenrollment().await
        }
        #[tokio::test]
        async fn a_replacement_token_cannot_identify_another_machine_and_is_good_once_and_only_in_time()
         {
            orch_api_base::backplane::reenrollment_tests::__api_run_a_replacement_token_cannot_identify_another_machine_and_is_good_once_and_only_in_time().await
        }
        #[tokio::test]
        async fn retained_connector_and_node_directories_replace_only_their_bound_identity_and_keep_local_data()
         {
            orch_api_base::backplane::reenrollment_tests::__api_run_retained_connector_and_node_directories_replace_only_their_bound_identity_and_keep_local_data().await
        }
    }
    #[cfg(test)]
    pub mod rate_presence_tests {
        pub use orch_api_base::backplane::rate_presence_tests::*;
        #[tokio::test]
        async fn an_unmeasured_or_partial_link_stays_absent_through_report_and_machine_api() {
            orch_api_base::backplane::rate_presence_tests::__api_run_an_unmeasured_or_partial_link_stays_absent_through_report_and_machine_api().await
        }
    }
    #[cfg(test)]
    pub mod binding_tests {
        pub use orch_api_base::backplane::binding_tests::*;
        #[tokio::test]
        async fn a_sign_in_discovered_again_on_reconnect_keeps_one_provider_and_one_profile() {
            orch_api_base::backplane::binding_tests::__api_run_a_sign_in_discovered_again_on_reconnect_keeps_one_provider_and_one_profile().await
        }
        #[tokio::test]
        async fn discovery_defaults_to_the_machines_registered_home_and_preserves_a_moved_resource()
        {
            orch_api_base::backplane::binding_tests::__api_run_discovery_defaults_to_the_machines_registered_home_and_preserves_a_moved_resource().await
        }
        #[tokio::test]
        async fn an_owner_without_a_personal_home_keeps_a_registration_current_and_gets_no_new_one()
        {
            orch_api_base::backplane::binding_tests::__api_run_an_owner_without_a_personal_home_keeps_a_registration_current_and_gets_no_new_one().await
        }
        #[tokio::test]
        async fn a_profile_pointed_at_another_home_is_never_reused_for_the_default_sign_in() {
            orch_api_base::backplane::binding_tests::__api_run_a_profile_pointed_at_another_home_is_never_reused_for_the_default_sign_in().await
        }
        #[tokio::test]
        async fn a_moved_default_login_profile_is_reused_without_resetting_its_display_home() {
            orch_api_base::backplane::binding_tests::__api_run_a_moved_default_login_profile_is_reused_without_resetting_its_display_home().await
        }
    }

    pub mod port_records {
        pub use orch_api_base::backplane::port_records::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::backplane::port_records::tests::*;
            #[test]
            fn a_manager_gets_one_ordered_whole_row_page_with_both_owners_counts_and_the_earliest_measurement()
             {
                orch_api_base::backplane::port_records::tests::__api_run_a_manager_gets_one_ordered_whole_row_page_with_both_owners_counts_and_the_earliest_measurement()
            }
            #[test]
            fn a_missing_carrier_inventory_is_unavailable_instead_of_fabricated_zero() {
                orch_api_base::backplane::port_records::tests::__api_run_a_missing_carrier_inventory_is_unavailable_instead_of_fabricated_zero()
            }
        }
    }
    #[cfg(test)]
    pub mod session_source_tests {
        pub use orch_api_base::backplane::session_source_tests::*;

        pub mod port_creation_replays {
            pub use orch_api_base::backplane::session_source_tests::port_creation_replays::*;
            #[tokio::test]
            async fn signed_port_use_headers_keep_tcp_and_udp_admissions_without_a_second_proof_or_native_observation()
             {
                orch_api_base::backplane::session_source_tests::port_creation_replays::__api_run_signed_port_use_headers_keep_tcp_and_udp_admissions_without_a_second_proof_or_native_observation().await
            }
        }
        #[tokio::test]
        async fn a_foreign_session_report_is_acknowledged_before_ordering_without_raw_or_a_cursor_even_after_reconnect()
         {
            orch_api_base::backplane::session_source_tests::__api_run_a_foreign_session_report_is_acknowledged_before_ordering_without_raw_or_a_cursor_even_after_reconnect().await
        }
        #[tokio::test]
        async fn an_unbound_segment_accepts_nobody_until_the_server_assigns_its_source() {
            orch_api_base::backplane::session_source_tests::__api_run_an_unbound_segment_accepts_nobody_until_the_server_assigns_its_source().await
        }
        #[tokio::test]
        async fn a_foreign_session_reset_writes_no_cursor_and_sends_no_resync_while_its_source_can_reset()
         {
            orch_api_base::backplane::session_source_tests::__api_run_a_foreign_session_reset_writes_no_cursor_and_sends_no_resync_while_its_source_can_reset().await
        }
    }
    #[cfg(test)]
    pub mod machine_report_tests {
        pub use orch_api_base::backplane::machine_report_tests::*;
        #[tokio::test]
        async fn an_old_segments_machine_cannot_report_usage_for_a_new_segments_turn() {
            orch_api_base::backplane::machine_report_tests::__api_run_an_old_segments_machine_cannot_report_usage_for_a_new_segments_turn().await
        }
        #[tokio::test]
        async fn a_providers_machine_cannot_report_another_providers_lease() {
            orch_api_base::backplane::machine_report_tests::__api_run_a_providers_machine_cannot_report_another_providers_lease().await
        }
        #[tokio::test]
        async fn an_environment_cannot_close_another_environments_browser_or_terminal() {
            orch_api_base::backplane::machine_report_tests::__api_run_an_environment_cannot_close_another_environments_browser_or_terminal().await
        }
        #[tokio::test]
        async fn twenty_foreign_reports_are_acknowledged_counted_and_audited_once_without_content()
        {
            orch_api_base::backplane::machine_report_tests::__api_run_twenty_foreign_reports_are_acknowledged_counted_and_audited_once_without_content().await
        }
        #[tokio::test]
        async fn separate_foreign_frames_keep_the_hourly_audit_append_only_and_carry_the_pending_count()
         {
            orch_api_base::backplane::machine_report_tests::__api_run_separate_foreign_frames_keep_the_hourly_audit_append_only_and_carry_the_pending_count().await
        }
        #[tokio::test]
        async fn replacing_an_api_process_does_not_write_a_second_refusal_row_in_the_same_hour() {
            orch_api_base::backplane::machine_report_tests::__api_run_replacing_an_api_process_does_not_write_a_second_refusal_row_in_the_same_hour().await
        }
        #[tokio::test]
        async fn foreign_environment_facts_status_and_progress_are_dropped_and_the_source_is_taken()
        {
            orch_api_base::backplane::machine_report_tests::__api_run_foreign_environment_facts_status_and_progress_are_dropped_and_the_source_is_taken().await
        }
        #[tokio::test]
        async fn a_harness_and_provider_take_reports_only_from_their_registered_machine() {
            orch_api_base::backplane::machine_report_tests::__api_run_a_harness_and_provider_take_reports_only_from_their_registered_machine().await
        }
        #[tokio::test]
        async fn a_managed_profile_reports_from_a_requested_colocated_harness_and_a_pinned_profile_cannot_move()
         {
            orch_api_base::backplane::machine_report_tests::__api_run_a_managed_profile_reports_from_a_requested_colocated_harness_and_a_pinned_profile_cannot_move().await
        }
        #[tokio::test]
        async fn a_node_cannot_report_another_hosts_runtime_or_job_and_the_registered_node_can() {
            orch_api_base::backplane::machine_report_tests::__api_run_a_node_cannot_report_another_hosts_runtime_or_job_and_the_registered_node_can().await
        }
        #[tokio::test]
        async fn a_foreign_report_in_a_node_batch_applies_none_of_the_batch() {
            orch_api_base::backplane::machine_report_tests::__api_run_a_foreign_report_in_a_node_batch_applies_none_of_the_batch().await
        }
        #[tokio::test]
        async fn discarding_a_foreign_runtime_does_not_block_the_nodes_next_own_report() {
            orch_api_base::backplane::machine_report_tests::__api_run_discarding_a_foreign_runtime_does_not_block_the_nodes_next_own_report().await
        }
        #[tokio::test]
        async fn a_foreign_environment_measurement_is_never_published_and_the_source_is_published()
        {
            orch_api_base::backplane::machine_report_tests::__api_run_a_foreign_environment_measurement_is_never_published_and_the_source_is_published().await
        }
        #[tokio::test]
        async fn an_environment_stream_cannot_update_a_job_of_another_environment() {
            orch_api_base::backplane::machine_report_tests::__api_run_an_environment_stream_cannot_update_a_job_of_another_environment().await
        }
        #[tokio::test]
        async fn a_move_destination_reports_only_requested_work_and_its_facts_wait_for_commit() {
            orch_api_base::backplane::machine_report_tests::__api_run_a_move_destination_reports_only_requested_work_and_its_facts_wait_for_commit().await
        }
        #[tokio::test]
        async fn a_runtime_network_report_obeys_the_runtime_hosts_source_binding() {
            orch_api_base::backplane::machine_report_tests::__api_run_a_runtime_network_report_obeys_the_runtime_hosts_source_binding().await
        }
    }
    #[cfg(test)]
    pub mod command_admission_tests {
        pub use orch_api_base::backplane::command_admission_tests::*;

        pub mod ask_origins {
            pub use orch_api_base::backplane::command_admission_tests::ask_origins::*;
            #[tokio::test]
            async fn a_queued_ask_request_keeps_the_original_access_token_deadline_after_a_native_row_wait()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_a_queued_ask_request_keeps_the_original_access_token_deadline_after_a_native_row_wait().await
            }
            #[tokio::test]
            async fn a_queued_ask_command_rechecks_its_controller_model_endpoint_and_retained_request_before_delivery()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_a_queued_ask_command_rechecks_its_controller_model_endpoint_and_retained_request_before_delivery().await
            }
            #[tokio::test]
            async fn ordinary_members_cannot_change_another_persons_ask_preferences_or_delete_it()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_ordinary_members_cannot_change_another_persons_ask_preferences_or_delete_it().await
            }
            #[tokio::test]
            async fn ordinary_members_cannot_read_another_persons_ask_and_event_frames_keep_current_access()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_ordinary_members_cannot_read_another_persons_ask_and_event_frames_keep_current_access().await
            }
            #[tokio::test]
            async fn an_ordinary_member_cannot_stop_another_persons_ask_and_the_owner_can_stop_it_idle()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_an_ordinary_member_cannot_stop_another_persons_ask_and_the_owner_can_stop_it_idle().await
            }
            #[tokio::test]
            async fn explicit_root_inspection_reads_an_ask_without_permission_to_stop_it()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_explicit_root_inspection_reads_an_ask_without_permission_to_stop_it().await
            }
            #[tokio::test]
            async fn ask_creation_history_and_model_choices_require_current_access_to_the_selected_root()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_ask_creation_history_and_model_choices_require_current_access_to_the_selected_root().await
            }
            #[tokio::test]
            async fn submitting_an_ask_needs_its_own_history_read_beside_message_write()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_submitting_an_ask_needs_its_own_history_read_beside_message_write().await
            }
            #[tokio::test]
            async fn ordinary_members_cannot_add_or_open_another_persons_ask_attachment()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_ordinary_members_cannot_add_or_open_another_persons_ask_attachment().await
            }
            #[tokio::test]
            async fn keeping_a_temporary_ask_requires_its_current_owner()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_keeping_a_temporary_ask_requires_its_current_owner().await
            }
            #[tokio::test]
            async fn idle_ask_streams_read_once_then_wake_on_committed_events_and_resume_their_own_cursor()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_idle_ask_streams_read_once_then_wake_on_committed_events_and_resume_their_own_cursor().await
            }
            #[tokio::test]
            async fn the_shared_thread_sweep_repairs_an_ask_event_without_a_notification()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_the_shared_thread_sweep_repairs_an_ask_event_without_a_notification().await
            }
            #[tokio::test]
            async fn an_idle_ask_stream_ends_at_its_original_access_deadline_without_new_rows()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_an_idle_ask_stream_ends_at_its_original_access_deadline_without_new_rows().await
            }
            #[tokio::test]
            async fn an_idle_ask_stream_wakes_on_its_own_session_revocation_without_new_rows()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_an_idle_ask_stream_wakes_on_its_own_session_revocation_without_new_rows().await
            }
            #[tokio::test]
            async fn an_idle_ask_stream_rechecks_its_root_after_a_committed_permission_epoch_change()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::ask_origins::__api_run_an_idle_ask_stream_rechecks_its_root_after_a_committed_permission_epoch_change().await
            }
        }

        pub mod http_origins {
            pub use orch_api_base::backplane::command_admission_tests::http_origins::*;
            #[tokio::test]
            async fn raw_http_queue_checks_the_original_digest_after_the_real_entry_wait()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::http_origins::__api_run_raw_http_queue_checks_the_original_digest_after_the_real_entry_wait().await
            }
            #[tokio::test]
            async fn raw_http_redelivery_refuses_rotated_bearers_and_changed_physical_sources()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::http_origins::__api_run_raw_http_redelivery_refuses_rotated_bearers_and_changed_physical_sources().await
            }
            #[tokio::test]
            async fn raw_http_access_deadline_survives_extension_and_is_checked_after_the_actual_wire_wait()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::http_origins::__api_run_raw_http_access_deadline_survives_extension_and_is_checked_after_the_actual_wire_wait().await
            }
            #[tokio::test]
            async fn raw_http_explicit_mode_is_a_ceiling_and_identity_delivery_does_not_rerun_a_completed_source()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::http_origins::__api_run_raw_http_explicit_mode_is_a_ceiling_and_identity_delivery_does_not_rerun_a_completed_source().await
            }
            #[tokio::test]
            async fn raw_http_retains_an_accepted_scope_minimum_and_keeps_it_on_the_shared_response_opening()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::http_origins::__api_run_raw_http_retains_an_accepted_scope_minimum_and_keeps_it_on_the_shared_response_opening().await
            }
            #[tokio::test]
            async fn raw_http_modern_codec_rejects_missing_identity_and_a_native_caller_is_not_a_bearer()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::http_origins::__api_run_raw_http_modern_codec_rejects_missing_identity_and_a_native_caller_is_not_a_bearer().await
            }
            #[tokio::test]
            async fn a_native_http_deadline_keeps_its_actual_microseconds_across_issue_wire_and_receipt()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::http_origins::__api_run_a_native_http_deadline_keeps_its_actual_microseconds_across_issue_wire_and_receipt().await
            }
        }

        pub mod input_deliveries {
            pub use orch_api_base::backplane::command_admission_tests::input_deliveries::*;
            #[tokio::test]
            async fn queued_start_rechecks_its_actual_input_origin_before_first_wire()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::input_deliveries::__api_run_queued_start_rechecks_its_actual_input_origin_before_first_wire().await
            }
            #[tokio::test]
            async fn queued_fenced_start_rechecks_its_actual_input_origin_before_first_wire()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::input_deliveries::__api_run_queued_fenced_start_rechecks_its_actual_input_origin_before_first_wire().await
            }
            #[tokio::test]
            async fn queued_steer_rechecks_its_actual_input_origin_before_first_wire()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::input_deliveries::__api_run_queued_steer_rechecks_its_actual_input_origin_before_first_wire().await
            }
            #[tokio::test]
            async fn start_cannot_publish_after_key_expiry_during_the_final_wire_cut()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::input_deliveries::__api_run_start_cannot_publish_after_key_expiry_during_the_final_wire_cut().await
            }
            #[tokio::test]
            async fn fenced_start_cannot_publish_after_key_expiry_during_the_final_wire_cut()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::input_deliveries::__api_run_fenced_start_cannot_publish_after_key_expiry_during_the_final_wire_cut().await
            }
            #[tokio::test]
            async fn steer_cannot_publish_after_key_expiry_during_the_final_wire_cut()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::input_deliveries::__api_run_steer_cannot_publish_after_key_expiry_during_the_final_wire_cut().await
            }
            #[tokio::test]
            async fn an_actual_accepted_start_result_reconciles_after_its_origin_expires()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::input_deliveries::__api_run_an_actual_accepted_start_result_reconciles_after_its_origin_expires().await
            }
            #[tokio::test]
            async fn an_actual_accepted_fenced_start_result_reconciles_after_its_origin_expires()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::input_deliveries::__api_run_an_actual_accepted_fenced_start_result_reconciles_after_its_origin_expires().await
            }
            #[tokio::test]
            async fn an_actual_accepted_steer_result_reconciles_after_its_origin_expires()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::input_deliveries::__api_run_an_actual_accepted_steer_result_reconciles_after_its_origin_expires().await
            }
            #[tokio::test]
            async fn a_new_scoped_steer_cannot_adopt_or_cancel_an_old_queued_none_command()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::input_deliveries::__api_run_a_new_scoped_steer_cannot_adopt_or_cancel_an_old_queued_none_command().await
            }
            #[tokio::test]
            async fn a_new_scoped_steer_cannot_relabel_an_actual_accepted_none_result()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::input_deliveries::__api_run_a_new_scoped_steer_cannot_relabel_an_actual_accepted_none_result().await
            }
        }

        pub mod mcp_imports {
            pub use orch_api_base::backplane::command_admission_tests::mcp_imports::*;
            #[tokio::test]
            async fn imported_mcp_keeps_its_raw_bearer_at_actual_entry_and_wire()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::mcp_imports::__api_run_imported_mcp_keeps_its_raw_bearer_at_actual_entry_and_wire().await
            }
            #[tokio::test]
            async fn stored_mcp_reimport_and_offering_use_keep_the_same_raw_bearer_at_actual_entry()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::mcp_imports::__api_run_stored_mcp_reimport_and_offering_use_keep_the_same_raw_bearer_at_actual_entry().await
            }
            #[tokio::test]
            async fn native_import_inverse_owns_only_the_successful_unfiled_copy_after_the_original_key_ends()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::mcp_imports::__api_run_native_import_inverse_owns_only_the_successful_unfiled_copy_after_the_original_key_ends().await
            }
        }

        pub mod mount_locations {
            pub use orch_api_base::backplane::command_admission_tests::mount_locations::*;
            #[tokio::test]
            async fn mount_ensure_records_only_its_exact_accepted_volume_and_keeps_a_write_only_receipt_after_issuer_loss()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::mount_locations::__api_run_mount_ensure_records_only_its_exact_accepted_volume_and_keeps_a_write_only_receipt_after_issuer_loss().await
            }
            #[tokio::test]
            async fn mount_delete_removes_only_its_accepted_original_volume_and_completes_bookkeeping_after_issuer_loss()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::mount_locations::__api_run_mount_delete_removes_only_its_accepted_original_volume_and_completes_bookkeeping_after_issuer_loss().await
            }
            #[tokio::test]
            async fn kept_purge_uses_the_exact_retained_row_and_keeps_a_current_location_on_another_machine()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::mount_locations::__api_run_kept_purge_uses_the_exact_retained_row_and_keeps_a_current_location_on_another_machine().await
            }
        }

        pub mod provider_login {
            pub use orch_api_base::backplane::command_admission_tests::provider_login::*;

            pub mod provider_handover {
                pub use orch_api_base::backplane::command_admission_tests::provider_login::provider_handover::*;
                #[tokio::test]
                async fn queued_handover_return_and_accept_recheck_the_actual_key_before_first_reconnect_wire()
                -> impl ::std::process::Termination {
                    orch_api_base::backplane::command_admission_tests::provider_login::provider_handover::__api_run_queued_handover_return_and_accept_recheck_the_actual_key_before_first_reconnect_wire().await
                }
                #[tokio::test]
                async fn already_accepted_handover_results_remain_canonical_after_the_initiating_key_is_revoked()
                -> impl ::std::process::Termination {
                    orch_api_base::backplane::command_admission_tests::provider_login::provider_handover::__api_run_already_accepted_handover_results_remain_canonical_after_the_initiating_key_is_revoked().await
                }
                #[tokio::test]
                async fn a_succeeded_return_stays_for_its_live_custody_retry_and_is_reaped_after_release()
                -> impl ::std::process::Termination {
                    orch_api_base::backplane::command_admission_tests::provider_login::provider_handover::__api_run_a_succeeded_return_stays_for_its_live_custody_retry_and_is_reaped_after_release().await
                }
            }
            #[tokio::test]
            async fn a_queued_provider_start_rechecks_its_real_key_pending_attempt_and_setup_on_reconnect()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::provider_login::__api_run_a_queued_provider_start_rechecks_its_real_key_pending_attempt_and_setup_on_reconnect().await
            }
            #[tokio::test]
            async fn a_queued_provider_input_rechecks_both_real_keys_and_the_actual_active_attempt_on_reconnect()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::provider_login::__api_run_a_queued_provider_input_rechecks_both_real_keys_and_the_actual_active_attempt_on_reconnect().await
            }
            #[tokio::test]
            async fn a_queued_provider_cancel_uses_its_own_key_and_cannot_cancel_a_replacement_on_reconnect()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::provider_login::__api_run_a_queued_provider_cancel_uses_its_own_key_and_cannot_cancel_a_replacement_on_reconnect().await
            }
            #[tokio::test]
            async fn a_native_login_ack_before_its_database_mirror_survives_a_later_local_authority_refusal()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::provider_login::__api_run_a_native_login_ack_before_its_database_mirror_survives_a_later_local_authority_refusal().await
            }
        }

        pub mod publication {
            pub use orch_api_base::backplane::command_admission_tests::publication::*;
            #[tokio::test]
            async fn a_revoked_queued_commit_never_reaches_the_real_native_journal_on_reconnect()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::publication::__api_run_a_revoked_queued_commit_never_reaches_the_real_native_journal_on_reconnect().await
            }
            #[tokio::test]
            async fn a_revoked_queued_push_never_reaches_the_real_native_journal_on_reconnect()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::publication::__api_run_a_revoked_queued_push_never_reaches_the_real_native_journal_on_reconnect().await
            }
        }

        pub mod quota {
            pub use orch_api_base::backplane::command_admission_tests::quota::*;
            #[tokio::test]
            async fn ready_provider_complete_and_embed_reach_the_native_journal()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::quota::__api_run_ready_provider_complete_and_embed_reach_the_native_journal().await
            }
            #[tokio::test]
            async fn cached_quota_with_credits_does_not_cancel_new_complete_admission()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::quota::__api_run_cached_quota_with_credits_does_not_cancel_new_complete_admission().await
            }
            #[tokio::test]
            async fn cached_quota_with_credits_does_not_cancel_new_embed_admission()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::quota::__api_run_cached_quota_with_credits_does_not_cancel_new_embed_admission().await
            }
            #[tokio::test]
            async fn cached_quota_with_credits_does_not_cancel_complete_before_transmission()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::quota::__api_run_cached_quota_with_credits_does_not_cancel_complete_before_transmission().await
            }
            #[tokio::test]
            async fn cached_quota_with_credits_does_not_cancel_embed_before_transmission()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::quota::__api_run_cached_quota_with_credits_does_not_cancel_embed_before_transmission().await
            }
            #[tokio::test]
            async fn explicit_provider_disable_still_blocks_complete_and_embed()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::quota::__api_run_explicit_provider_disable_still_blocks_complete_and_embed().await
            }
            #[tokio::test]
            async fn explicit_model_disable_still_blocks_complete_and_embed()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::quota::__api_run_explicit_model_disable_still_blocks_complete_and_embed().await
            }
            #[tokio::test]
            async fn an_accepted_stop_still_blocks_complete_and_embed_after_the_provider_is_ready()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::quota::__api_run_an_accepted_stop_still_blocks_complete_and_embed_after_the_provider_is_ready().await
            }
        }

        pub mod reference_previews {
            pub use orch_api_base::backplane::command_admission_tests::reference_previews::*;
            #[tokio::test]
            async fn reference_live_stat_fetch_and_delivery_keep_raw_identity_physical_source_and_exactly_once_io()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::reference_previews::__api_run_reference_live_stat_fetch_and_delivery_keep_raw_identity_physical_source_and_exactly_once_io().await
            }
        }

        pub mod selection {
            pub use orch_api_base::backplane::command_admission_tests::selection::*;
            #[tokio::test]
            async fn a_real_guarded_selection_and_gc_can_finish_before_the_old_cancelled_dispatcher_adoption_without_a_late_effect()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::selection::__api_run_a_real_guarded_selection_and_gc_can_finish_before_the_old_cancelled_dispatcher_adoption_without_a_late_effect().await
            }
        }

        pub mod skill_creations {
            pub use orch_api_base::backplane::command_admission_tests::skill_creations::*;
            #[tokio::test]
            async fn native_skill_sources_keep_actual_environment_or_host_raw_origin_and_once_only_supplier_work()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::skill_creations::__api_run_native_skill_sources_keep_actual_environment_or_host_raw_origin_and_once_only_supplier_work().await
            }
        }

        pub mod snapshot_reads {
            pub use orch_api_base::backplane::command_admission_tests::snapshot_reads::*;
            #[tokio::test]
            async fn ordinary_snapshot_http_read_has_an_actual_native_journal_result()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::snapshot_reads::__api_run_ordinary_snapshot_http_read_has_an_actual_native_journal_result().await
            }
            #[tokio::test]
            async fn revoked_queued_snapshot_http_read_never_reaches_native_accepted_after_reconnect()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::snapshot_reads::__api_run_revoked_queued_snapshot_http_read_never_reaches_native_accepted_after_reconnect().await
            }
            #[tokio::test]
            async fn queued_snapshot_read_cannot_rebind_a_new_custodian_generation_to_its_old_wire_command()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::snapshot_reads::__api_run_queued_snapshot_read_cannot_rebind_a_new_custodian_generation_to_its_old_wire_command().await
            }
        }

        pub mod terminal_close {
            pub use orch_api_base::backplane::command_admission_tests::terminal_close::*;
            #[tokio::test]
            async fn new_scoped_terminal_close_refuses_unaccepted_legacy_none_without_cancelling_old_work()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::terminal_close::__api_run_new_scoped_terminal_close_refuses_unaccepted_legacy_none_without_cancelling_old_work().await
            }
            #[tokio::test]
            async fn new_scoped_attempt_never_relabels_accepted_legacy_none_or_vetoes_its_result()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::terminal_close::__api_run_new_scoped_attempt_never_relabels_accepted_legacy_none_or_vetoes_its_result().await
            }
            #[tokio::test]
            async fn captured_terminal_close_rechecks_its_real_key_before_first_wire()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::terminal_close::__api_run_captured_terminal_close_rechecks_its_real_key_before_first_wire().await
            }
            #[tokio::test]
            async fn captured_terminal_close_rechecks_its_actual_environment_generation_before_first_wire()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::terminal_close::__api_run_captured_terminal_close_rechecks_its_actual_environment_generation_before_first_wire().await
            }
        }

        pub mod transport {
            pub use orch_api_base::backplane::command_admission_tests::transport::*;
            #[tokio::test]
            async fn two_forwarded_turns_use_the_production_command_port_and_rebind_without_a_durable_api_body()
            -> impl ::std::process::Termination {
                orch_api_base::backplane::command_admission_tests::transport::__api_run_two_forwarded_turns_use_the_production_command_port_and_rebind_without_a_durable_api_body().await
            }
        }
        #[tokio::test]
        async fn an_already_connected_dispatcher_adopts_a_retained_turn_start_only_when_issued()
        -> impl ::std::process::Termination {
            orch_api_base::backplane::command_admission_tests::__api_run_an_already_connected_dispatcher_adopts_a_retained_turn_start_only_when_issued().await
        }
        #[tokio::test]
        async fn a_cancelled_queued_row_cannot_be_adopted_after_its_actual_native_completion_probe()
        -> impl ::std::process::Termination {
            orch_api_base::backplane::command_admission_tests::__api_run_a_cancelled_queued_row_cannot_be_adopted_after_its_actual_native_completion_probe().await
        }
        #[tokio::test]
        async fn a_stale_startup_open_row_cannot_resurrect_after_cancellation_before_entry_publication()
        -> impl ::std::process::Termination {
            orch_api_base::backplane::command_admission_tests::__api_run_a_stale_startup_open_row_cannot_resurrect_after_cancellation_before_entry_publication().await
        }
        #[tokio::test]
        async fn a_probe_after_cancellation_cannot_pass_the_write_already_owned_by_its_output_iteration()
        -> impl ::std::process::Termination {
            orch_api_base::backplane::command_admission_tests::__api_run_a_probe_after_cancellation_cannot_pass_the_write_already_owned_by_its_output_iteration().await
        }
        #[tokio::test]
        async fn a_retired_output_task_cannot_publish_after_the_current_entry_has_completed_its_probe()
        -> impl ::std::process::Termination {
            orch_api_base::backplane::command_admission_tests::__api_run_a_retired_output_task_cannot_publish_after_the_current_entry_has_completed_its_probe().await
        }
        #[tokio::test]
        async fn only_an_explicit_live_observation_owner_may_use_its_existing_non_durable_read_route()
        -> impl ::std::process::Termination {
            orch_api_base::backplane::command_admission_tests::__api_run_only_an_explicit_live_observation_owner_may_use_its_existing_non_durable_read_route().await
        }
    }
}

pub mod blobs {
    pub use orch_api_base::blobs::*;
    #[cfg(test)]
    pub mod tests {
        pub use orch_api_base::blobs::tests::*;
        #[tokio::test]
        async fn a_staged_request_is_unpublished_and_its_body_owns_the_temporary_file_until_consumed()
         {
            orch_api_base::blobs::tests::__api_run_a_staged_request_is_unpublished_and_its_body_owns_the_temporary_file_until_consumed().await
        }
        #[tokio::test]
        async fn preparing_a_piece_keeps_existing_bytes_until_the_native_write_is_admitted() {
            orch_api_base::blobs::tests::__api_run_preparing_a_piece_keeps_existing_bytes_until_the_native_write_is_admitted().await
        }
        #[tokio::test]
        async fn a_machine_runs_four_transfers_at_once_and_a_fifth_waits_for_one_to_end() {
            orch_api_base::blobs::tests::__api_run_a_machine_runs_four_transfers_at_once_and_a_fifth_waits_for_one_to_end().await
        }
        #[tokio::test]
        async fn a_body_is_stored_under_the_hash_it_has_and_one_that_is_too_large_leaves_nothing() {
            orch_api_base::blobs::tests::__api_run_a_body_is_stored_under_the_hash_it_has_and_one_that_is_too_large_leaves_nothing().await
        }
    }
}

pub mod cache {
    pub use orch_api_base::cache::*;
    #[cfg(test)]
    pub mod tests {
        pub use orch_api_base::cache::tests::*;
        #[tokio::test]
        async fn a_job_runs_in_one_process_at_a_time_under_its_lease_and_keeps_no_transaction_open()
        {
            orch_api_base::cache::tests::__api_run_a_job_runs_in_one_process_at_a_time_under_its_lease_and_keeps_no_transaction_open().await
        }
    }
}

pub mod certificates {
    pub use orch_api_base::certificates::*;

    pub mod frontend {
        pub use orch_api_base::certificates::frontend::*;
        #[cfg(all(test, unix))]
        pub mod tests {
            pub use orch_api_base::certificates::frontend::tests::*;
            #[test]
            fn the_existing_authority_issues_and_renews_the_front_with_its_names_as_a_complete_pair()
             {
                orch_api_base::certificates::frontend::tests::__api_run_the_existing_authority_issues_and_renews_the_front_with_its_names_as_a_complete_pair()
            }
        }
    }

    pub mod revocation {
        pub use orch_api_base::certificates::revocation::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::certificates::revocation::tests::*;
            #[tokio::test]
            async fn rejected_revocation_cut_leaves_registration_commands_and_audit_unchanged() {
                orch_api_base::certificates::revocation::tests::__api_run_rejected_revocation_cut_leaves_registration_commands_and_audit_unchanged().await
            }
            #[tokio::test]
            async fn revocation_packet_keeps_its_transaction_and_native_once_only_cancellation() {
                orch_api_base::certificates::revocation::tests::__api_run_revocation_packet_keeps_its_transaction_and_native_once_only_cancellation().await
            }
        }
    }
    #[cfg(test)]
    pub mod prepared_host_tokens {
        pub use orch_api_base::certificates::prepared_host_tokens::*;
        #[tokio::test]
        async fn a_host_token_cut_precedes_its_ledger_and_keeps_exact_choices_on_success() {
            orch_api_base::certificates::prepared_host_tokens::__api_run_a_host_token_cut_precedes_its_ledger_and_keeps_exact_choices_on_success().await
        }
        #[tokio::test]
        async fn a_host_token_reservation_cannot_cross_its_transaction_or_authority_before_the_cut()
        {
            orch_api_base::certificates::prepared_host_tokens::__api_run_a_host_token_reservation_cannot_cross_its_transaction_or_authority_before_the_cut().await
        }
    }
}

pub mod content {
    pub use orch_api_base::content::*;
    #[cfg(test)]
    #[test]
    fn native_result_uploads_require_the_exact_capture_command_and_typed_opt_in() {
        orch_api_base::content::__api_run_native_result_uploads_require_the_exact_capture_command_and_typed_opt_in()
    }
}

pub mod dispatch {
    pub use orch_api_base::dispatch::*;
    #[cfg(test)]
    pub mod tests {
        pub use orch_api_base::dispatch::tests::*;
        #[test]
        fn a_stored_command_carries_its_key_and_gives_it_back() {
            orch_api_base::dispatch::tests::__api_run_a_stored_command_carries_its_key_and_gives_it_back()
        }
        #[test]
        fn a_command_without_a_key_has_none() {
            orch_api_base::dispatch::tests::__api_run_a_command_without_a_key_has_none()
        }
        #[tokio::test]
        async fn a_ticket_request_in_a_command_is_signed_when_the_command_is_sent() {
            orch_api_base::dispatch::tests::__api_run_a_ticket_request_in_a_command_is_signed_when_the_command_is_sent().await
        }
        #[tokio::test]
        async fn a_secret_that_the_settings_stored_is_sealed_to_the_machine_for_the_command_that_names_it()
         {
            orch_api_base::dispatch::tests::__api_run_a_secret_that_the_settings_stored_is_sealed_to_the_machine_for_the_command_that_names_it().await
        }
        #[tokio::test]
        async fn a_command_that_cannot_be_made_whole_is_not_sent_and_says_why() {
            orch_api_base::dispatch::tests::__api_run_a_command_that_cannot_be_made_whole_is_not_sent_and_says_why().await
        }
        #[tokio::test]
        async fn the_answer_of_a_read_is_cleared_once_had_and_a_late_reader_is_told_that_it_expired()
         {
            orch_api_base::dispatch::tests::__api_run_the_answer_of_a_read_is_cleared_once_had_and_a_late_reader_is_told_that_it_expired().await
        }
    }
}

pub mod documents_upkeep {
    pub use orch_api_base::documents_upkeep::*;
    #[cfg(test)]
    pub mod tests {
        pub use orch_api_base::documents_upkeep::tests::*;
        #[test]
        fn a_span_longer_than_time_can_say_reaches_back_to_the_beginning_and_never_fails() {
            orch_api_base::documents_upkeep::tests::__api_run_a_span_longer_than_time_can_say_reaches_back_to_the_beginning_and_never_fails()
        }
    }
}

pub mod harness_channels {
    pub use orch_api_base::harness_channels::*;

    pub mod github {
        pub use orch_api_base::harness_channels::github::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::harness_channels::github::tests::*;
            #[test]
            fn releases_give_their_published_linux_packages_with_the_checksums_github_keeps() {
                orch_api_base::harness_channels::github::tests::__api_run_releases_give_their_published_linux_packages_with_the_checksums_github_keeps()
            }
            #[test]
            fn a_checksum_file_of_the_release_stands_for_the_digest_where_the_manifest_says_so() {
                orch_api_base::harness_channels::github::tests::__api_run_a_checksum_file_of_the_release_stands_for_the_digest_where_the_manifest_says_so()
            }
        }
    }

    pub mod npm {
        pub use orch_api_base::harness_channels::npm::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::harness_channels::npm::tests::*;
            #[test]
            fn a_version_counts_only_when_one_of_the_registrys_keys_verifies_its_signature() {
                orch_api_base::harness_channels::npm::tests::__api_run_a_version_counts_only_when_one_of_the_registrys_keys_verifies_its_signature()
            }
            #[test]
            fn the_tags_version_comes_first_then_the_newest_without_pre_releases() {
                orch_api_base::harness_channels::npm::tests::__api_run_the_tags_version_comes_first_then_the_newest_without_pre_releases()
            }
            #[test]
            fn a_scoped_package_is_asked_for_with_its_slash_written_out() {
                orch_api_base::harness_channels::npm::tests::__api_run_a_scoped_package_is_asked_for_with_its_slash_written_out()
            }
        }
    }

    pub mod pypi {
        pub use orch_api_base::harness_channels::pypi::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::harness_channels::pypi::tests::*;
            #[test]
            fn a_release_gives_a_wheel_for_each_processor_with_the_checksum_the_index_keeps() {
                orch_api_base::harness_channels::pypi::tests::__api_run_a_release_gives_a_wheel_for_each_processor_with_the_checksum_the_index_keeps()
            }
            #[test]
            fn a_release_the_index_names_in_pep_440_is_kept_under_the_name_a_profile_pins() {
                orch_api_base::harness_channels::pypi::tests::__api_run_a_release_the_index_names_in_pep_440_is_kept_under_the_name_a_profile_pins()
            }
            #[test]
            fn an_exact_release_reads_the_files_the_index_lists_for_it() {
                orch_api_base::harness_channels::pypi::tests::__api_run_an_exact_release_reads_the_files_the_index_lists_for_it()
            }
        }
    }

    pub mod signed {
        pub use orch_api_base::harness_channels::signed::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::harness_channels::signed::tests::*;
            #[test]
            fn a_manifest_gives_its_linux_programs_with_their_checksums() {
                orch_api_base::harness_channels::signed::tests::__api_run_a_manifest_gives_its_linux_programs_with_their_checksums()
            }
            #[test]
            fn a_vendors_signature_counts_only_when_it_verifies() {
                orch_api_base::harness_channels::signed::tests::__api_run_a_vendors_signature_counts_only_when_it_verifies()
            }
        }
    }
    #[cfg(test)]
    pub mod tests {
        pub use orch_api_base::harness_channels::tests::*;
        #[test]
        fn old_channel_rows_have_no_checksum_authority_and_mirror_trust_is_explicit() {
            orch_api_base::harness_channels::tests::__api_run_old_channel_rows_have_no_checksum_authority_and_mirror_trust_is_explicit()
        }
        #[test]
        fn codex_releases_give_their_published_linux_packages_with_the_checksums_github_keeps() {
            orch_api_base::harness_channels::tests::__api_run_codex_releases_give_their_published_linux_packages_with_the_checksums_github_keeps()
        }
        #[test]
        fn a_claude_code_manifest_gives_its_linux_programs_with_their_checksums() {
            orch_api_base::harness_channels::tests::__api_run_a_claude_code_manifest_gives_its_linux_programs_with_their_checksums()
        }
        #[test]
        fn a_mirror_serves_each_vendor_under_its_host() {
            orch_api_base::harness_channels::tests::__api_run_a_mirror_serves_each_vendor_under_its_host()
        }
        #[test]
        fn a_pattern_may_stand_for_what_varies_in_a_file_name() {
            orch_api_base::harness_channels::tests::__api_run_a_pattern_may_stand_for_what_varies_in_a_file_name()
        }
        #[test]
        fn a_build_that_is_kept_is_known_by_its_key_for_its_version_and_processor_alone() {
            orch_api_base::harness_channels::tests::__api_run_a_build_that_is_kept_is_known_by_its_key_for_its_version_and_processor_alone()
        }
        #[test]
        fn the_programs_orch_ships_name_a_channel_each_but_the_fake() {
            orch_api_base::harness_channels::tests::__api_run_the_programs_orch_ships_name_a_channel_each_but_the_fake()
        }
    }
}

pub mod host_ports {
    pub use orch_api_base::host_ports::*;
    #[cfg(test)]
    pub mod tests {
        pub use orch_api_base::host_ports::tests::*;
        #[test]
        fn applications_never_share_proposals_and_a_cancelled_read_can_be_tried_again() {
            orch_api_base::host_ports::tests::__api_run_applications_never_share_proposals_and_a_cancelled_read_can_be_tried_again()
        }
    }
}

pub mod hosting {
    pub use orch_api_base::hosting::*;
    #[cfg(test)]
    pub mod upgrade_tests {
        pub use orch_api_base::hosting::upgrade_tests::*;
        #[test]
        fn every_program_resolves_shipped_checked_newest_and_exact_without_retargeting_a_recorded_pin()
         {
            orch_api_base::hosting::upgrade_tests::__api_run_every_program_resolves_shipped_checked_newest_and_exact_without_retargeting_a_recorded_pin()
        }
        #[test]
        fn checksum_refusal_remains_a_refusal_for_every_program_and_policy() {
            orch_api_base::hosting::upgrade_tests::__api_run_checksum_refusal_remains_a_refusal_for_every_program_and_policy()
        }
        #[test]
        fn mixed_machine_versions_are_independent_while_one_unit_cannot_satisfy_conflicting_pins() {
            orch_api_base::hosting::upgrade_tests::__api_run_mixed_machine_versions_are_independent_while_one_unit_cannot_satisfy_conflicting_pins()
        }
        #[test]
        fn a_python_index_pre_release_pin_keeps_the_canonical_existing_build_identity() {
            orch_api_base::hosting::upgrade_tests::__api_run_a_python_index_pre_release_pin_keeps_the_canonical_existing_build_identity()
        }
    }
    #[cfg(test)]
    pub mod tests {
        pub use orch_api_base::hosting::tests::*;
        #[test]
        fn a_profile_pin_selects_the_listed_nondefault_build_and_never_falls_back() {
            orch_api_base::hosting::tests::__api_run_a_profile_pin_selects_the_listed_nondefault_build_and_never_falls_back()
        }
        #[test]
        fn a_place_asking_for_the_newest_gets_the_newest_checked_or_any_and_never_a_pre_release_order_by_text()
         {
            orch_api_base::hosting::tests::__api_run_a_place_asking_for_the_newest_gets_the_newest_checked_or_any_and_never_a_pre_release_order_by_text()
        }
        #[test]
        fn a_release_is_after_its_pre_releases_and_pre_releases_go_by_kind_and_number() {
            orch_api_base::hosting::tests::__api_run_a_release_is_after_its_pre_releases_and_pre_releases_go_by_kind_and_number()
        }
        #[test]
        fn the_builds_for_a_platform_are_those_listed_for_it_with_the_connector_first() {
            orch_api_base::hosting::tests::__api_run_the_builds_for_a_platform_are_those_listed_for_it_with_the_connector_first()
        }
        #[test]
        fn the_builds_of_a_unit_are_the_connector_then_the_harnesses_each_name_once() {
            orch_api_base::hosting::tests::__api_run_the_builds_of_a_unit_are_the_connector_then_the_harnesses_each_name_once()
        }
        #[tokio::test]
        async fn a_unit_runs_the_connector_of_the_release_at_the_address_anybody_downloads_it_from()
        {
            orch_api_base::hosting::tests::__api_run_a_unit_runs_the_connector_of_the_release_at_the_address_anybody_downloads_it_from().await
        }
        #[tokio::test]
        async fn a_connector_that_was_replaced_is_another_version_with_a_hash_of_its_own() {
            orch_api_base::hosting::tests::__api_run_a_connector_that_was_replaced_is_another_version_with_a_hash_of_its_own().await
        }
        #[tokio::test]
        async fn a_system_that_was_not_started_from_a_release_has_no_connector_to_put_in_a_unit() {
            orch_api_base::hosting::tests::__api_run_a_system_that_was_not_started_from_a_release_has_no_connector_to_put_in_a_unit().await
        }
        #[tokio::test]
        async fn a_release_without_a_connector_for_the_platform_of_the_host_has_nothing_to_put_in_a_unit()
         {
            orch_api_base::hosting::tests::__api_run_a_release_without_a_connector_for_the_platform_of_the_host_has_nothing_to_put_in_a_unit().await
        }
        #[test]
        fn a_platform_without_a_connector_build_has_nothing_to_put_in_a_unit() {
            orch_api_base::hosting::tests::__api_run_a_platform_without_a_connector_build_has_nothing_to_put_in_a_unit()
        }
    }
}

pub mod input_schedules {
    pub use orch_api_base::input_schedules::*;

    pub mod cron {
        pub use orch_api_base::input_schedules::cron::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::input_schedules::cron::tests::*;
            #[test]
            fn explicit_cron_steps_bounds_and_calendar_rules_have_stable_occurrences() {
                orch_api_base::input_schedules::cron::tests::__api_run_explicit_cron_steps_bounds_and_calendar_rules_have_stable_occurrences()
            }
        }
    }

    pub mod timing {
        pub use orch_api_base::input_schedules::timing::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::input_schedules::timing::tests::*;
            #[test]
            fn a_gap_is_a_visible_skip_and_a_fold_has_only_the_earlier_occurrence() {
                orch_api_base::input_schedules::timing::tests::__api_run_a_gap_is_a_visible_skip_and_a_fold_has_only_the_earlier_occurrence()
            }
            #[test]
            fn a_backlog_coalesces_and_resume_chooses_only_future_work() {
                orch_api_base::input_schedules::timing::tests::__api_run_a_backlog_coalesces_and_resume_chooses_only_future_work()
            }
        }
    }
}

pub(crate) mod key_operations {
    pub use orch_api_base::key_operations::*;

    pub mod activity_store {
        pub use orch_api_base::key_operations::activity_store::*;
        #[cfg(test)]
        #[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
        pub mod tests {
            pub use orch_api_base::key_operations::activity_store::tests::*;
            #[tokio::test]
            async fn the_shared_context_keeps_explicit_replay_and_outcome_once_without_raw_inputs()
            {
                orch_api_base::key_operations::activity_store::tests::__api_run_the_shared_context_keeps_explicit_replay_and_outcome_once_without_raw_inputs().await
            }
        }
    }

    pub mod admin {
        pub use orch_api_base::key_operations::admin::*;
        #[cfg(test)]
        #[allow(clippy::unwrap_used, reason = "test assertions fail by panicking")]
        pub mod tests {
            pub use orch_api_base::key_operations::admin::tests::*;
            #[test]
            fn browser_mode_and_current_standing_are_preserved() {
                orch_api_base::key_operations::admin::tests::__api_run_browser_mode_and_current_standing_are_preserved()
            }
            #[test]
            fn exact_system_read_is_admitted_without_private_override() {
                orch_api_base::key_operations::admin::tests::__api_run_exact_system_read_is_admitted_without_private_override()
            }
            #[test]
            fn targeted_administration_does_not_require_an_artificial_system_target() {
                orch_api_base::key_operations::admin::tests::__api_run_targeted_administration_does_not_require_an_artificial_system_target()
            }
            #[test]
            fn legacy_or_unadmitted_credentials_do_not_gain_admin_operations() {
                orch_api_base::key_operations::admin::tests::__api_run_legacy_or_unadmitted_credentials_do_not_gain_admin_operations()
            }
            #[test]
            fn administration_accepts_real_empty_roles_and_keeps_optional_read_on_current_readmission()
             {
                orch_api_base::key_operations::admin::tests::__api_run_administration_accepts_real_empty_roles_and_keeps_optional_read_on_current_readmission()
            }
            #[test]
            fn mixed_empty_administration_requires_each_canonical_role_not_a_matching_flat_target()
            {
                orch_api_base::key_operations::admin::tests::__api_run_mixed_empty_administration_requires_each_canonical_role_not_a_matching_flat_target()
            }
            #[test]
            fn node_settings_proof_never_covers_another_node_or_an_environment_provider_or_private_need()
             {
                orch_api_base::key_operations::admin::tests::__api_run_node_settings_proof_never_covers_another_node_or_an_environment_provider_or_private_need()
            }
        }
    }

    pub mod admission {
        pub use orch_api_base::key_operations::admission::*;
        #[cfg(test)]
        pub mod origin_tests {
            pub use orch_api_base::key_operations::admission::origin_tests::*;
            #[test]
            fn a_wider_incoming_key_cannot_replace_the_restored_native_source_boundary() {
                orch_api_base::key_operations::admission::origin_tests::__api_run_a_wider_incoming_key_cannot_replace_the_restored_native_source_boundary()
            }
            #[test]
            fn narrowed_native_admission_retains_both_keys_and_never_adds_private_mode() {
                orch_api_base::key_operations::admission::origin_tests::__api_run_narrowed_native_admission_retains_both_keys_and_never_adds_private_mode()
            }
            #[test]
            fn a_restored_different_account_is_not_a_valid_native_operation_boundary() {
                orch_api_base::key_operations::admission::origin_tests::__api_run_a_restored_different_account_is_not_a_valid_native_operation_boundary()
            }
            #[test]
            fn an_adoption_identity_prefix_retains_the_initial_narrower_credential_bounds() {
                orch_api_base::key_operations::admission::origin_tests::__api_run_an_adoption_identity_prefix_retains_the_initial_narrower_credential_bounds()
            }
        }
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::key_operations::admission::tests::*;
            #[test]
            fn a_topic_prefix_never_satisfies_the_normal_complete_operation_gate() {
                orch_api_base::key_operations::admission::tests::__api_run_a_topic_prefix_never_satisfies_the_normal_complete_operation_gate()
            }
            #[test]
            fn a_thread_identity_prefix_cannot_skip_its_dependent_creation_effects() {
                orch_api_base::key_operations::admission::tests::__api_run_a_thread_identity_prefix_cannot_skip_its_dependent_creation_effects()
            }
            #[test]
            fn required_effects_admit_before_a_future_response_object_exists() {
                orch_api_base::key_operations::admission::tests::__api_run_required_effects_admit_before_a_future_response_object_exists()
            }
            #[test]
            fn only_an_explicit_collection_can_be_checked_empty_and_missing_still_refuses() {
                orch_api_base::key_operations::admission::tests::__api_run_only_an_explicit_collection_can_be_checked_empty_and_missing_still_refuses()
            }
            #[test]
            fn a_fully_empty_footprint_retains_exact_required_selection_evidence() {
                orch_api_base::key_operations::admission::tests::__api_run_a_fully_empty_footprint_retains_exact_required_selection_evidence()
            }
        }
    }

    pub mod direct {
        pub use orch_api_base::key_operations::direct::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::key_operations::direct::tests::*;
            #[test]
            fn untyped_paths_dynamic_sources_and_compound_variants_remain_closed() {
                orch_api_base::key_operations::direct::tests::__api_run_untyped_paths_dynamic_sources_and_compound_variants_remain_closed()
            }
            #[test]
            fn empty_wire_reply_is_a_receipt_with_optional_error_disclosure() {
                orch_api_base::key_operations::direct::tests::__api_run_empty_wire_reply_is_a_receipt_with_optional_error_disclosure()
            }
            #[test]
            fn explicit_parent_kind_is_independent_of_business_family_and_parameter_must_exist() {
                orch_api_base::key_operations::direct::tests::__api_run_explicit_parent_kind_is_independent_of_business_family_and_parameter_must_exist()
            }
        }
    }

    pub mod native {
        pub use orch_api_base::key_operations::native::*;

        pub mod effects {
            pub use orch_api_base::key_operations::native::effects::*;
            #[cfg(test)]
            pub mod common_initial_tests {
                pub use orch_api_base::key_operations::native::effects::common_initial_tests::*;

                pub mod transactions {
                    pub use orch_api_base::key_operations::native::effects::common_initial_tests::transactions::*;
                    #[tokio::test]
                    async fn preliminary_effect_preparation_runs_its_real_http_checkpoint_on_the_same_connection_without_committing_trials()
                    -> impl ::std::process::Termination {
                        orch_api_base::key_operations::native::effects::common_initial_tests::transactions::__api_run_preliminary_effect_preparation_runs_its_real_http_checkpoint_on_the_same_connection_without_committing_trials().await
                    }
                    #[tokio::test]
                    async fn effect_preparation_keeps_the_callers_existing_transaction_and_rolls_back_only_with_its_owner()
                    -> impl ::std::process::Termination {
                        orch_api_base::key_operations::native::effects::common_initial_tests::transactions::__api_run_effect_preparation_keeps_the_callers_existing_transaction_and_rolls_back_only_with_its_owner().await
                    }
                    #[tokio::test]
                    async fn a_refused_preliminary_checkpoint_leaves_no_trial_and_does_not_erase_current_scope_or_raw_lifetime()
                    -> impl ::std::process::Termination {
                        orch_api_base::key_operations::native::effects::common_initial_tests::transactions::__api_run_a_refused_preliminary_checkpoint_leaves_no_trial_and_does_not_erase_current_scope_or_raw_lifetime().await
                    }
                }
                #[tokio::test]
                async fn common_initial_opens_real_upload_once_for_concurrent_callers_and_pure_repreparation()
                -> impl ::std::process::Termination {
                    orch_api_base::key_operations::native::effects::common_initial_tests::__api_run_common_initial_opens_real_upload_once_for_concurrent_callers_and_pure_repreparation().await
                }
                #[tokio::test]
                async fn common_initial_rechecks_raw_identity_and_keeps_original_access_deadline_after_native_wait()
                -> impl ::std::process::Termination {
                    orch_api_base::key_operations::native::effects::common_initial_tests::__api_run_common_initial_rechecks_raw_identity_and_keeps_original_access_deadline_after_native_wait().await
                }
                #[tokio::test]
                async fn common_initial_keeps_observed_key_narrowing_even_when_policy_broadens_before_resolution()
                -> impl ::std::process::Termination {
                    orch_api_base::key_operations::native::effects::common_initial_tests::__api_run_common_initial_keeps_observed_key_narrowing_even_when_policy_broadens_before_resolution().await
                }
                #[tokio::test]
                async fn common_initial_does_not_retry_a_failed_native_source_open()
                -> impl ::std::process::Termination {
                    orch_api_base::key_operations::native::effects::common_initial_tests::__api_run_common_initial_does_not_retry_a_failed_native_source_open().await
                }
                #[tokio::test]
                async fn common_initial_keeps_the_ordinary_upload_custody_gate_before_native_io()
                -> impl ::std::process::Termination {
                    orch_api_base::key_operations::native::effects::common_initial_tests::__api_run_common_initial_keeps_the_ordinary_upload_custody_gate_before_native_io().await
                }
                #[tokio::test]
                async fn common_preparation_rechecks_native_custody_without_a_captured_operation()
                -> impl ::std::process::Termination {
                    orch_api_base::key_operations::native::effects::common_initial_tests::__api_run_common_preparation_rechecks_native_custody_without_a_captured_operation().await
                }
                #[tokio::test]
                async fn common_preparation_keeps_observed_policy_and_raw_lifetime_minima()
                -> impl ::std::process::Termination {
                    orch_api_base::key_operations::native::effects::common_initial_tests::__api_run_common_preparation_keeps_observed_policy_and_raw_lifetime_minima().await
                }
            }
            #[cfg(test)]
            pub mod entry_tests {
                pub use orch_api_base::key_operations::native::effects::entry_tests::*;
                #[tokio::test]
                async fn declared_adoption_entry_checks_real_prerequisites_without_admitting_future_creator_roles()
                -> impl ::std::process::Termination {
                    orch_api_base::key_operations::native::effects::entry_tests::__api_run_declared_adoption_entry_checks_real_prerequisites_without_admitting_future_creator_roles().await
                }
                #[tokio::test]
                async fn declared_entry_rejects_each_missing_duplicate_mistyped_or_undeclared_source_before_require()
                -> impl ::std::process::Termination {
                    orch_api_base::key_operations::native::effects::entry_tests::__api_run_declared_entry_rejects_each_missing_duplicate_mistyped_or_undeclared_source_before_require().await
                }
                #[tokio::test]
                async fn declared_entry_keeps_the_initial_origin_current_scope_and_actual_raw_lifetime()
                -> impl ::std::process::Termination {
                    orch_api_base::key_operations::native::effects::entry_tests::__api_run_declared_entry_keeps_the_initial_origin_current_scope_and_actual_raw_lifetime().await
                }
                #[tokio::test]
                async fn declared_entry_never_uses_scope_in_place_of_ordinary_authority_or_a_second_step_hold()
                -> impl ::std::process::Termination {
                    orch_api_base::key_operations::native::effects::entry_tests::__api_run_declared_entry_never_uses_scope_in_place_of_ordinary_authority_or_a_second_step_hold().await
                }
            }
        }

        pub mod reply {
            pub use orch_api_base::key_operations::native::reply::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_base::key_operations::native::reply::tests::*;
                #[test]
                fn a_completed_account_replay_requires_its_declared_response_source_and_no_spent_creation_source()
                -> impl ::std::process::Termination {
                    orch_api_base::key_operations::native::reply::tests::__api_run_a_completed_account_replay_requires_its_declared_response_source_and_no_spent_creation_source()
                }
                #[test]
                fn a_first_account_creation_without_its_declared_required_source_is_refused() {
                    orch_api_base::key_operations::native::reply::tests::__api_run_a_first_account_creation_without_its_declared_required_source_is_refused()
                }
                #[test]
                fn additional_native_response_roles_are_explicit_and_cannot_include_management()
                -> impl ::std::process::Termination {
                    orch_api_base::key_operations::native::reply::tests::__api_run_additional_native_response_roles_are_explicit_and_cannot_include_management()
                }
                #[test]
                fn retained_collection_subjects_are_order_independent_but_not_replaceable() {
                    orch_api_base::key_operations::native::reply::tests::__api_run_retained_collection_subjects_are_order_independent_but_not_replaceable()
                }
            }
        }

        pub mod resource_reply {
            pub use orch_api_base::key_operations::native::resource_reply::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_base::key_operations::native::resource_reply::tests::*;
                #[test]
                fn collection_read_needs_every_actual_subject_and_never_readmits_completed_manage()
                {
                    orch_api_base::key_operations::native::resource_reply::tests::__api_run_collection_read_needs_every_actual_subject_and_never_readmits_completed_manage()
                }
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
        pub mod tests {
            pub use orch_api_base::key_operations::native::tests::*;
            #[test]
            fn stored_declaration_keeps_manage_and_response_read_independent() {
                orch_api_base::key_operations::native::tests::__api_run_stored_declaration_keeps_manage_and_response_read_independent()
            }
            #[test]
            fn missing_sources_and_wrong_cardinality_never_become_empty_proof() {
                orch_api_base::key_operations::native::tests::__api_run_missing_sources_and_wrong_cardinality_never_become_empty_proof()
            }
            #[test]
            fn one_native_role_binds_all_of_its_canonical_required_actions() {
                orch_api_base::key_operations::native::tests::__api_run_one_native_role_binds_all_of_its_canonical_required_actions()
            }
            #[test]
            fn undeclared_business_kind_and_duplicate_source_are_rejected() {
                orch_api_base::key_operations::native::tests::__api_run_undeclared_business_kind_and_duplicate_source_are_rejected()
            }
            #[test]
            fn known_preparation_does_not_replace_an_unresolved_required_collection() {
                orch_api_base::key_operations::native::tests::__api_run_known_preparation_does_not_replace_an_unresolved_required_collection()
            }
            #[test]
            fn deferred_response_binding_keeps_required_subjects_complete_and_optional_sources_unproven()
             {
                orch_api_base::key_operations::native::tests::__api_run_deferred_response_binding_keeps_required_subjects_complete_and_optional_sources_unproven()
            }
        }
    }

    pub mod publication {
        pub use orch_api_base::key_operations::publication::*;
        #[cfg(test)]
        #[allow(clippy::unwrap_used, reason = "a control fails by panicking")]
        pub mod tests {
            pub use orch_api_base::key_operations::publication::tests::*;
            #[test]
            fn an_unaccepted_refusal_never_replaces_an_accepted_receipt_or_a_different_error() {
                orch_api_base::key_operations::publication::tests::__api_run_an_unaccepted_refusal_never_replaces_an_accepted_receipt_or_a_different_error()
            }
            #[test]
            fn sparse_failure_keeps_native_status_code_retry_and_trusted_cause() {
                orch_api_base::key_operations::publication::tests::__api_run_sparse_failure_keeps_native_status_code_retry_and_trusted_cause()
            }
        }
    }

    pub mod resolver {
        pub use orch_api_base::key_operations::resolver::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::key_operations::resolver::tests::*;
            #[test]
            fn raw_query_keeps_the_real_byte_body_and_native_offset_decoder() {
                orch_api_base::key_operations::resolver::tests::__api_run_raw_query_keeps_the_real_byte_body_and_native_offset_decoder()
            }
            #[test]
            fn raw_query_rejects_an_invented_body_read_mode_or_unbounded_retained_query() {
                orch_api_base::key_operations::resolver::tests::__api_run_raw_query_rejects_an_invented_body_read_mode_or_unbounded_retained_query()
            }
            #[test]
            fn retained_opened_source_survives_fresh_preparation_and_never_retargets() {
                orch_api_base::key_operations::resolver::tests::__api_run_retained_opened_source_survives_fresh_preparation_and_never_retargets()
            }
            #[test]
            fn poisoned_opened_source_cannot_downgrade_to_unopened() {
                orch_api_base::key_operations::resolver::tests::__api_run_poisoned_opened_source_cannot_downgrade_to_unopened()
            }
            #[test]
            fn unclassified_ordinary_path_never_admits_a_modern_key_or_retains_operation_proof() {
                orch_api_base::key_operations::resolver::tests::__api_run_unclassified_ordinary_path_never_admits_a_modern_key_or_retains_operation_proof()
            }
            #[test]
            fn pending_completion_is_explicit_and_denied_never_widens() {
                orch_api_base::key_operations::resolver::tests::__api_run_pending_completion_is_explicit_and_denied_never_widens()
            }
            #[test]
            fn prepared_operands_are_redacted_and_guard_snapshots_do_not_follow_later_replacement()
            {
                orch_api_base::key_operations::resolver::tests::__api_run_prepared_operands_are_redacted_and_guard_snapshots_do_not_follow_later_replacement()
            }
        }
    }

    pub mod targets {
        pub use orch_api_base::key_operations::targets::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::key_operations::targets::tests::*;
            #[test]
            fn typed_current_ancestry_comes_from_the_graph_and_same_id_does_not_change_kind() {
                orch_api_base::key_operations::targets::tests::__api_run_typed_current_ancestry_comes_from_the_graph_and_same_id_does_not_change_kind()
            }
            #[test]
            fn resource_home_is_placement_even_when_its_site_has_no_node_path() {
                orch_api_base::key_operations::targets::tests::__api_run_resource_home_is_placement_even_when_its_site_has_no_node_path()
            }
            #[test]
            fn document_uses_its_actual_beside_node_without_inventing_folder_ancestry() {
                orch_api_base::key_operations::targets::tests::__api_run_document_uses_its_actual_beside_node_without_inventing_folder_ancestry()
            }
            #[test]
            fn creation_family_does_not_relabel_its_real_parent() {
                orch_api_base::key_operations::targets::tests::__api_run_creation_family_does_not_relabel_its_real_parent()
            }
            #[test]
            fn bindings_cannot_silently_replace_resolved_targets() {
                orch_api_base::key_operations::targets::tests::__api_run_bindings_cannot_silently_replace_resolved_targets()
            }
        }
    }
    #[cfg(test)]
    pub mod completed_projection_tests {
        pub use orch_api_base::key_operations::completed_projection_tests::*;
        #[tokio::test]
        async fn completed_projection_provider_update_keeps_accepted_revision_after_real_snapshot_retry()
         {
            orch_api_base::key_operations::completed_projection_tests::__api_run_completed_projection_provider_update_keeps_accepted_revision_after_real_snapshot_retry().await
        }
        #[tokio::test]
        async fn completed_projection_model_create_keeps_accepted_model_after_real_snapshot_retry()
        {
            orch_api_base::key_operations::completed_projection_tests::__api_run_completed_projection_model_create_keeps_accepted_model_after_real_snapshot_retry().await
        }
        #[tokio::test]
        async fn completed_projection_actual_http_session_expiry_withholds_full_but_acknowledges_write()
         {
            orch_api_base::key_operations::completed_projection_tests::__api_run_completed_projection_actual_http_session_expiry_withholds_full_but_acknowledges_write().await
        }
    }
}

pub mod local_ports {
    pub use orch_api_base::local_ports::*;
    #[cfg(test)]
    pub mod pending_tests {
        pub use orch_api_base::local_ports::pending_tests::*;
        #[test]
        fn pending_read_admission_is_per_environment_and_cancellation_releases_its_room() {
            orch_api_base::local_ports::pending_tests::__api_run_pending_read_admission_is_per_environment_and_cancellation_releases_its_room()
        }
    }
}

pub mod media {
    pub use orch_api_base::media::*;
    #[cfg(test)]
    pub mod tests {
        pub use orch_api_base::media::tests::*;
        #[test]
        fn a_name_decides_the_type_and_the_first_bytes_decide_where_the_name_does_not() {
            orch_api_base::media::tests::__api_run_a_name_decides_the_type_and_the_first_bytes_decide_where_the_name_does_not()
        }
        #[test]
        fn content_is_text_when_it_holds_no_nul_and_reads_as_utf8() {
            orch_api_base::media::tests::__api_run_content_is_text_when_it_holds_no_nul_and_reads_as_utf8()
        }
        #[test]
        fn a_name_for_a_path_has_no_separator_and_no_leading_dot() {
            orch_api_base::media::tests::__api_run_a_name_for_a_path_has_no_separator_and_no_leading_dot()
        }
        #[test]
        fn active_content_is_not_shown_inline() {
            orch_api_base::media::tests::__api_run_active_content_is_not_shown_inline()
        }
    }
}

pub mod middleware {
    pub use orch_api_base::middleware::*;

    pub mod authentication {
        pub use orch_api_base::middleware::authentication::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::middleware::authentication::tests::*;
            #[test]
            fn surface_headers_reject_foreign_kinds_before_public_and_cookie_shortcuts() {
                orch_api_base::middleware::authentication::tests::__api_run_surface_headers_reject_foreign_kinds_before_public_and_cookie_shortcuts()
            }
            #[test]
            fn a_cookie_is_found_among_others() {
                orch_api_base::middleware::authentication::tests::__api_run_a_cookie_is_found_among_others()
            }
            #[test]
            fn tokens_are_long_random_and_told_apart_by_their_prefix() {
                orch_api_base::middleware::authentication::tests::__api_run_tokens_are_long_random_and_told_apart_by_their_prefix()
            }
        }
    }

    pub mod authorization {
        pub use orch_api_base::middleware::authorization::*;
        #[cfg(test)]
        pub mod projection_tests {
            pub use orch_api_base::middleware::authorization::projection_tests::*;
            #[test]
            fn limited_problem_keeps_code_and_retry_without_named_payload() {
                orch_api_base::middleware::authorization::projection_tests::__api_run_limited_problem_keeps_code_and_retry_without_named_payload()
            }
            #[test]
            fn limited_problem_preserves_trusted_retry_without_trusting_a_public_lookalike() {
                orch_api_base::middleware::authorization::projection_tests::__api_run_limited_problem_preserves_trusted_retry_without_trusting_a_public_lookalike()
            }
        }
    }

    pub mod idempotency {
        pub use orch_api_base::middleware::idempotency::*;

        pub mod commands {
            pub use orch_api_base::middleware::idempotency::commands::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_base::middleware::idempotency::commands::tests::*;
                #[tokio::test]
                async fn each_native_creating_command_is_atomic_with_its_exact_data_only_reference()
                -> impl ::std::process::Termination {
                    orch_api_base::middleware::idempotency::commands::tests::__api_run_each_native_creating_command_is_atomic_with_its_exact_data_only_reference().await
                }
                #[tokio::test]
                async fn a_native_preparation_of_the_same_kind_does_not_claim_the_later_creation()
                -> impl ::std::process::Termination {
                    orch_api_base::middleware::idempotency::commands::tests::__api_run_a_native_preparation_of_the_same_kind_does_not_claim_the_later_creation().await
                }
            }
        }

        pub mod memory {
            pub use orch_api_base::middleware::idempotency::memory::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_base::middleware::idempotency::memory::tests::*;
                #[test]
                fn only_publishing_keeps_the_native_memory_entry() {
                    orch_api_base::middleware::idempotency::memory::tests::__api_run_only_publishing_keeps_the_native_memory_entry()
                }
            }
        }
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::middleware::idempotency::tests::*;
            #[tokio::test]
            async fn partial_native_progress_resumes_under_the_same_key_and_a_completed_outcome_stays_final()
            -> impl ::std::process::Termination {
                orch_api_base::middleware::idempotency::tests::__api_run_partial_native_progress_resumes_under_the_same_key_and_a_completed_outcome_stays_final().await
            }
            #[tokio::test]
            async fn a_completed_creation_never_installs_the_original_writer_request()
            -> impl ::std::process::Termination {
                orch_api_base::middleware::idempotency::tests::__api_run_a_completed_creation_never_installs_the_original_writer_request().await
            }
            #[tokio::test]
            async fn an_identity_free_consumer_without_its_observer_fails_closed_instead_of_running_twice()
            -> impl ::std::process::Termination {
                orch_api_base::middleware::idempotency::tests::__api_run_an_identity_free_consumer_without_its_observer_fails_closed_instead_of_running_twice().await
            }
        }
    }

    pub mod inspection {
        pub use orch_api_base::middleware::inspection::*;
        #[cfg(test)]
        pub mod snapshot_tests {
            pub use orch_api_base::middleware::inspection::snapshot_tests::*;
            #[test]
            fn successful_snapshot_keeps_exact_deletion_and_read_scope_without_auto_upgrade() {
                orch_api_base::middleware::inspection::snapshot_tests::__api_run_successful_snapshot_keeps_exact_deletion_and_read_scope_without_auto_upgrade()
            }
        }
    }

    pub mod requests {
        pub use orch_api_base::middleware::requests::*;

        pub mod words {
            pub use orch_api_base::middleware::requests::words::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_base::middleware::requests::words::tests::*;
                #[test]
                fn extras_cannot_replace_contract_fields_or_publish_internal_diagnostics() {
                    orch_api_base::middleware::requests::words::tests::__api_run_extras_cannot_replace_contract_fields_or_publish_internal_diagnostics()
                }
                #[test]
                fn field_diagnostics_are_withheld_without_losing_the_field_or_useful_explanation() {
                    orch_api_base::middleware::requests::words::tests::__api_run_field_diagnostics_are_withheld_without_losing_the_field_or_useful_explanation()
                }
            }
        }
        #[cfg(test)]
        pub mod check_tests {
            pub use orch_api_base::middleware::requests::check_tests::*;
            #[test]
            fn a_value_the_schemas_check_refuses_is_the_requests_fault_at_its_field() {
                orch_api_base::middleware::requests::check_tests::__api_run_a_value_the_schemas_check_refuses_is_the_requests_fault_at_its_field()
            }
            #[test]
            fn a_known_native_source_refusal_is_an_authorization_answer_without_an_internal_error()
            {
                orch_api_base::middleware::requests::check_tests::__api_run_a_known_native_source_refusal_is_an_authorization_answer_without_an_internal_error()
            }
            #[test]
            fn only_a_typed_database_conflict_marks_an_effect_free_read_for_retry() {
                orch_api_base::middleware::requests::check_tests::__api_run_only_a_typed_database_conflict_marks_an_effect_free_read_for_retry()
            }
            #[test]
            fn an_engine_commit_that_lost_a_race_is_the_callers_to_send_again_and_nothing_else_is()
            {
                orch_api_base::middleware::requests::check_tests::__api_run_an_engine_commit_that_lost_a_race_is_the_callers_to_send_again_and_nothing_else_is()
            }
        }
    }

    pub mod transaction_guard {
        pub use orch_api_base::middleware::transaction_guard::*;
        #[cfg(test)]
        pub mod deadline_tests {
            pub use orch_api_base::middleware::transaction_guard::deadline_tests::*;
            #[tokio::test]
            async fn original_raw_and_accepted_key_deadlines_survive_a_real_native_preparation_wait()
             {
                orch_api_base::middleware::transaction_guard::deadline_tests::__api_run_original_raw_and_accepted_key_deadlines_survive_a_real_native_preparation_wait().await
            }
        }
    }
}

pub mod port_tools {
    pub use orch_api_base::port_tools::*;
    #[cfg(test)]
    pub mod budget_tests {
        pub use orch_api_base::port_tools::budget_tests::*;
        #[test]
        fn supplied_marks_and_multibyte_text_count_before_a_change_is_allowed() {
            orch_api_base::port_tools::budget_tests::__api_run_supplied_marks_and_multibyte_text_count_before_a_change_is_allowed()
        }
    }
}

pub mod ports_browser {
    pub use orch_api_base::ports_browser::*;
    #[cfg(test)]
    pub mod tests {
        pub use orch_api_base::ports_browser::tests::*;
        #[test]
        fn orch_s_cookie_and_the_link_token_never_reach_the_program() {
            orch_api_base::ports_browser::tests::__api_run_orch_s_cookie_and_the_link_token_never_reach_the_program()
        }
        #[test]
        fn a_caller_s_own_identity_headers_are_taken_out_in_every_case_and_copy() {
            orch_api_base::ports_browser::tests::__api_run_a_caller_s_own_identity_headers_are_taken_out_in_every_case_and_copy()
        }
        #[test]
        fn a_display_name_travels_as_percent_text() {
            orch_api_base::ports_browser::tests::__api_run_a_display_name_travels_as_percent_text()
        }
    }
}

pub mod ports_runtime {
    pub use orch_api_base::ports_runtime::*;
    #[cfg(test)]
    pub mod surface_tests {
        pub use orch_api_base::ports_runtime::surface_tests::*;
        #[tokio::test]
        async fn surface_port_use_refusal_preserves_the_same_proof_and_origin() {
            orch_api_base::ports_runtime::surface_tests::__api_run_surface_port_use_refusal_preserves_the_same_proof_and_origin().await
        }
    }
}

pub mod previews {
    pub use orch_api_base::previews::*;
    #[cfg(test)]
    pub mod tests {
        pub use orch_api_base::previews::tests::*;
        #[test]
        fn text_cards_share_the_reference_card_bound_even_for_one_very_long_line() {
            orch_api_base::previews::tests::__api_run_text_cards_share_the_reference_card_bound_even_for_one_very_long_line()
        }
        #[test]
        fn active_original_types_are_inert_downloads_from_the_shared_file_server() {
            orch_api_base::previews::tests::__api_run_active_original_types_are_inert_downloads_from_the_shared_file_server()
        }
        #[test]
        fn previews_are_bounded_static_images_and_keep_transparency() {
            orch_api_base::previews::tests::__api_run_previews_are_bounded_static_images_and_keep_transparency()
        }
        #[test]
        fn an_image_over_the_pixel_budget_is_refused_before_decode() {
            orch_api_base::previews::tests::__api_run_an_image_over_the_pixel_budget_is_refused_before_decode()
        }
        #[test]
        fn the_static_preview_applies_the_original_exif_orientation() {
            orch_api_base::previews::tests::__api_run_the_static_preview_applies_the_original_exif_orientation()
        }
        #[test]
        fn an_animated_image_produces_only_its_first_frame_as_a_png() {
            orch_api_base::previews::tests::__api_run_an_animated_image_produces_only_its_first_frame_as_a_png()
        }
        #[test]
        fn svg_is_rasterized_without_resolving_external_images_or_running_scripts() {
            orch_api_base::previews::tests::__api_run_svg_is_rasterized_without_resolving_external_images_or_running_scripts()
        }
        #[test]
        fn the_cache_is_byte_bounded_and_discards_the_least_recently_used_preview() {
            orch_api_base::previews::tests::__api_run_the_cache_is_byte_bounded_and_discards_the_least_recently_used_preview()
        }
        #[tokio::test]
        async fn a_reader_leaving_does_not_repeat_the_same_original_read_or_lose_its_cached_preview()
         {
            orch_api_base::previews::tests::__api_run_a_reader_leaving_does_not_repeat_the_same_original_read_or_lose_its_cached_preview().await
        }
    }
}

pub mod programs {
    pub use orch_api_base::programs::*;
    #[cfg(test)]
    pub mod tests {
        pub use orch_api_base::programs::tests::*;
        #[test]
        fn the_offers_of_an_executor_are_its_sets_namespaced_with_the_old_word_as_their_alias() {
            orch_api_base::programs::tests::__api_run_the_offers_of_an_executor_are_its_sets_namespaced_with_the_old_word_as_their_alias()
        }
        #[test]
        fn an_external_executors_report_carries_localized_tool_set_text_without_identifier_fallback()
         {
            orch_api_base::programs::tests::__api_run_an_external_executors_report_carries_localized_tool_set_text_without_identifier_fallback()
        }
        #[test]
        fn the_check_of_a_program_is_titled_case_by_case_and_feature_by_feature() {
            orch_api_base::programs::tests::__api_run_the_check_of_a_program_is_titled_case_by_case_and_feature_by_feature()
        }
    }
}

pub mod realtime {
    pub use orch_api_base::realtime::*;
    #[cfg(test)]
    pub mod background_read_tests {
        pub use orch_api_base::realtime::background_read_tests::*;
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn http_session_expiry_after_native_preparation_withholds_the_roster()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::background_read_tests::__api_run_http_session_expiry_after_native_preparation_withholds_the_roster().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn http_legacy_key_expiry_after_native_preparation_withholds_the_roster()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::background_read_tests::__api_run_http_legacy_key_expiry_after_native_preparation_withholds_the_roster().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn http_scoped_key_expiry_after_native_preparation_withholds_the_roster()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::background_read_tests::__api_run_http_scoped_key_expiry_after_native_preparation_withholds_the_roster().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn http_nonexpiring_key_remains_valid_after_native_preparation()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::background_read_tests::__api_run_http_nonexpiring_key_remains_valid_after_native_preparation().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn http_second_page_reprojects_an_expired_optional_source_without_native_recapture()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::background_read_tests::__api_run_http_second_page_reprojects_an_expired_optional_source_without_native_recapture().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn http_native_wait_uses_the_actual_session_mode_without_borrowing_another_session()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::background_read_tests::__api_run_http_native_wait_uses_the_actual_session_mode_without_borrowing_another_session().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn socket_initial_seed_reprojects_optional_source_expiry_without_an_epoch_change()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::background_read_tests::__api_run_socket_initial_seed_reprojects_optional_source_expiry_without_an_epoch_change().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn socket_broadcast_reprojects_optional_source_expiry_without_an_epoch_change()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::background_read_tests::__api_run_socket_broadcast_reprojects_optional_source_expiry_without_an_epoch_change().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn socket_raw_session_expiry_withholds_a_projected_roster_and_closes()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::background_read_tests::__api_run_socket_raw_session_expiry_withholds_a_projected_roster_and_closes().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn http_background_uses_its_actual_second_step_hold_after_source_preparation()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::background_read_tests::__api_run_http_background_uses_its_actual_second_step_hold_after_source_preparation().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn socket_background_closes_its_feed_on_actual_second_step_loss_without_borrowing_peer_proof()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::background_read_tests::__api_run_socket_background_closes_its_feed_on_actual_second_step_loss_without_borrowing_peer_proof().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn indirect_role_expiry_reprojects_provider_directories_from_the_native_holder_stamp()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::background_read_tests::__api_run_indirect_role_expiry_reprojects_provider_directories_from_the_native_holder_stamp().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn native_background_wrapper_preserves_scoped_admission_and_current_key_revocation()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::background_read_tests::__api_run_native_background_wrapper_preserves_scoped_admission_and_current_key_revocation().await
        }
    }

    pub mod browsers {
        pub use orch_api_base::realtime::browsers::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::realtime::browsers::tests::*;
            #[test]
            fn surface_ticket_foreign_redemption_cannot_consume_either_origin() {
                orch_api_base::realtime::browsers::tests::__api_run_surface_ticket_foreign_redemption_cannot_consume_either_origin()
            }
            #[test]
            fn file_navigation_and_workspace_chooser_paths_need_the_files_door() {
                orch_api_base::realtime::browsers::tests::__api_run_file_navigation_and_workspace_chooser_paths_need_the_files_door()
            }
            #[test]
            fn a_sitting_keeps_only_the_host_of_a_visited_address() {
                orch_api_base::realtime::browsers::tests::__api_run_a_sitting_keeps_only_the_host_of_a_visited_address()
            }
        }
    }

    pub mod deltas {
        pub use orch_api_base::realtime::deltas::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::realtime::deltas::tests::*;
            #[test]
            fn deltas_that_wait_for_a_slow_reader_become_one_larger_delta_and_lose_nothing() {
                orch_api_base::realtime::deltas::tests::__api_run_deltas_that_wait_for_a_slow_reader_become_one_larger_delta_and_lose_nothing()
            }
            #[test]
            fn a_reader_who_takes_each_frame_as_it_comes_is_sent_each_delta_as_it_grew() {
                orch_api_base::realtime::deltas::tests::__api_run_a_reader_who_takes_each_frame_as_it_comes_is_sent_each_delta_as_it_grew()
            }
            #[test]
            fn the_deltas_of_two_items_are_not_mixed() {
                orch_api_base::realtime::deltas::tests::__api_run_the_deltas_of_two_items_are_not_mixed()
            }
            #[test]
            fn a_delta_that_does_not_continue_the_one_that_waits_goes_behind_it() {
                orch_api_base::realtime::deltas::tests::__api_run_a_delta_that_does_not_continue_the_one_that_waits_goes_behind_it()
            }
            #[test]
            fn a_row_that_holds_all_that_a_waiting_delta_says_makes_the_delta_pointless() {
                orch_api_base::realtime::deltas::tests::__api_run_a_row_that_holds_all_that_a_waiting_delta_says_makes_the_delta_pointless()
            }
            #[test]
            fn a_row_that_holds_only_the_start_of_a_waiting_delta_goes_behind_it_and_what_grows_next_is_a_delta_of_its_own()
             {
                orch_api_base::realtime::deltas::tests::__api_run_a_row_that_holds_only_the_start_of_a_waiting_delta_goes_behind_it_and_what_grows_next_is_a_delta_of_its_own()
            }
            #[test]
            fn a_row_older_than_a_waiting_delta_leaves_it_in_place_to_grow() {
                orch_api_base::realtime::deltas::tests::__api_run_a_row_older_than_a_waiting_delta_leaves_it_in_place_to_grow()
            }
            #[test]
            fn rows_are_never_dropped_and_leave_in_the_order_they_were_queued() {
                orch_api_base::realtime::deltas::tests::__api_run_rows_are_never_dropped_and_leave_in_the_order_they_were_queued()
            }
            #[test]
            fn an_answer_to_a_ping_leaves_before_what_waits() {
                orch_api_base::realtime::deltas::tests::__api_run_an_answer_to_a_ping_leaves_before_what_waits()
            }
            #[test]
            fn what_an_ended_subscription_left_in_the_queue_is_taken_out_and_the_others_are_not() {
                orch_api_base::realtime::deltas::tests::__api_run_what_an_ended_subscription_left_in_the_queue_is_taken_out_and_the_others_are_not()
            }
            #[test]
            fn past_the_limit_deltas_and_events_are_shed_and_each_subscription_is_told_to_read_again_once()
             {
                orch_api_base::realtime::deltas::tests::__api_run_past_the_limit_deltas_and_events_are_shed_and_each_subscription_is_told_to_read_again_once()
            }
            #[test]
            fn the_limit_is_for_what_waits_and_a_reader_that_keeps_taking_frames_is_never_over_it()
            {
                orch_api_base::realtime::deltas::tests::__api_run_the_limit_is_for_what_waits_and_a_reader_that_keeps_taking_frames_is_never_over_it()
            }
            #[test]
            fn a_connection_that_takes_nothing_is_closed_with_4408_after_the_time_and_not_before() {
                orch_api_base::realtime::deltas::tests::__api_run_a_connection_that_takes_nothing_is_closed_with_4408_after_the_time_and_not_before()
            }
            #[test]
            fn a_frame_in_the_sockets_hands_counts_as_waiting() {
                orch_api_base::realtime::deltas::tests::__api_run_a_frame_in_the_sockets_hands_counts_as_waiting()
            }
            #[test]
            fn frames_queued_after_the_connection_was_closed_are_not_kept() {
                orch_api_base::realtime::deltas::tests::__api_run_frames_queued_after_the_connection_was_closed_are_not_kept()
            }
            #[tokio::test]
            async fn the_writer_waits_for_a_frame_and_is_woken_by_it() {
                orch_api_base::realtime::deltas::tests::__api_run_the_writer_waits_for_a_frame_and_is_woken_by_it().await
            }
        }
    }

    pub mod displays {
        pub use orch_api_base::realtime::displays::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::realtime::displays::tests::*;
            #[test]
            fn surface_ticket_foreign_redemption_cannot_consume_either_origin() {
                orch_api_base::realtime::displays::tests::__api_run_surface_ticket_foreign_redemption_cannot_consume_either_origin()
            }
            #[test]
            fn a_viewer_is_asked_for_no_password_even_when_its_messages_are_split() {
                orch_api_base::realtime::displays::tests::__api_run_a_viewer_is_asked_for_no_password_even_when_its_messages_are_split()
            }
            #[test]
            fn a_false_security_choice_ends_the_connection_instead_of_becoming_input() {
                orch_api_base::realtime::displays::tests::__api_run_a_false_security_choice_ends_the_connection_instead_of_becoming_input()
            }
        }
    }

    pub mod ephemeral {
        pub use orch_api_base::realtime::ephemeral::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::realtime::ephemeral::tests::*;
            #[tokio::test]
            async fn watching_a_thousand_machines_among_a_million_reads_only_indexed_batches() {
                orch_api_base::realtime::ephemeral::tests::__api_run_watching_a_thousand_machines_among_a_million_reads_only_indexed_batches().await
            }
            #[test]
            fn a_departed_watch_stays_for_ten_seconds_and_a_return_cancels_its_departure() {
                orch_api_base::realtime::ephemeral::tests::__api_run_a_departed_watch_stays_for_ten_seconds_and_a_return_cancels_its_departure()
            }
            #[test]
            fn ten_lists_in_one_second_leave_only_the_last_one_to_be_sent() {
                orch_api_base::realtime::ephemeral::tests::__api_run_ten_lists_in_one_second_leave_only_the_last_one_to_be_sent()
            }
            #[test]
            fn partial_directory_invalidation_retains_rows_but_cannot_reuse_a_snapshot_cursor() {
                orch_api_base::realtime::ephemeral::tests::__api_run_partial_directory_invalidation_retains_rows_but_cannot_reuse_a_snapshot_cursor()
            }
            #[test]
            fn a_removed_file_cancels_an_earlier_change_and_a_recreated_file_cancels_its_removal() {
                orch_api_base::realtime::ephemeral::tests::__api_run_a_removed_file_cancels_an_earlier_change_and_a_recreated_file_cancels_its_removal()
            }
        }
    }
    #[cfg(test)]
    pub mod epoch_handoff_tests {
        pub use orch_api_base::realtime::epoch_handoff_tests::*;

        pub mod native_feed_keys {
            pub use orch_api_base::realtime::epoch_handoff_tests::native_feed_keys::*;
            #[tokio::test]
            async fn raw_manage_key_reads_the_same_segment_over_http_and_live_delivery_then_revocation_stops_it()
            -> impl ::std::process::Termination {
                orch_api_base::realtime::epoch_handoff_tests::native_feed_keys::__api_run_raw_manage_key_reads_the_same_segment_over_http_and_live_delivery_then_revocation_stops_it().await
            }
            #[tokio::test]
            async fn raw_scope_does_not_supply_ordinary_management_or_admit_a_foreign_segment()
            -> impl ::std::process::Termination {
                orch_api_base::realtime::epoch_handoff_tests::native_feed_keys::__api_run_raw_scope_does_not_supply_ordinary_management_or_admit_a_foreign_segment().await
            }
            #[tokio::test]
            async fn background_read_uses_the_same_provider_directory_scope_for_http_and_delivery()
            -> impl ::std::process::Termination {
                orch_api_base::realtime::epoch_handoff_tests::native_feed_keys::__api_run_background_read_uses_the_same_provider_directory_scope_for_http_and_delivery().await
            }
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn notice_first_preserves_the_native_enrollment_status_and_purges_old_inbox_rows()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_notice_first_preserves_the_native_enrollment_status_and_purges_old_inbox_rows().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn authority_recheck_first_preserves_the_same_native_issuer_notice()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_authority_recheck_first_preserves_the_same_native_issuer_notice().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn generic_personal_state_stays_current_after_repeated_inbox_resets_and_never_revives_a_closed_reader()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_generic_personal_state_stays_current_after_repeated_inbox_resets_and_never_revives_a_closed_reader().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_revoked_exact_session_cannot_receive_personal_news_while_its_other_session_still_can()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_a_revoked_exact_session_cannot_receive_personal_news_while_its_other_session_still_can().await
        }
        #[test]
        fn native_row_failure_diagnostics_keep_dynamic_keys_and_values_opaque() {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_native_row_failure_diagnostics_keep_dynamic_keys_and_values_opaque()
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn deleting_an_environment_during_initial_projection_closes_only_that_acknowledged_reader()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_deleting_an_environment_during_initial_projection_closes_only_that_acknowledged_reader().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn an_invalid_projection_on_a_live_environment_remains_an_error_not_a_deleted_reader()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_an_invalid_projection_on_a_live_environment_remains_an_error_not_a_deleted_reader().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn native_resource_manage_withdrawal_purges_private_progress_then_last_right_withdrawal_closes_the_reader()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_native_resource_manage_withdrawal_purges_private_progress_then_last_right_withdrawal_closes_the_reader().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_purged_unsent_native_resource_replacement_is_reprojected_but_private_progress_does_not_break_quietness()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_a_purged_unsent_native_resource_replacement_is_reprojected_but_private_progress_does_not_break_quietness().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_changed_personal_projection_cut_reprojects_the_notice_before_publication()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_a_changed_personal_projection_cut_reprojects_the_notice_before_publication().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_credential_deadline_crossed_during_paused_inbox_projection_prevents_publication()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_a_credential_deadline_crossed_during_paused_inbox_projection_prevents_publication().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_manage_withdrawal_after_native_resource_projection_cannot_publish_the_old_rich_access()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_a_manage_withdrawal_after_native_resource_projection_cannot_publish_the_old_rich_access().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn purging_one_resource_generation_keeps_the_other_views_queued_replacement_and_fingerprint()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_purging_one_resource_generation_keeps_the_other_views_queued_replacement_and_fingerprint().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn metadata_hints_use_existing_personal_readers_and_keep_exact_session_fences()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_metadata_hints_use_existing_personal_readers_and_keep_exact_session_fences().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_nodes_machine_feed_keeps_host_view_until_the_physical_machine_is_deleted()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_a_nodes_machine_feed_keeps_host_view_until_the_physical_machine_is_deleted().await
        }
        #[tokio::test]
        async fn an_existing_scoped_key_authenticates_realtime_as_that_key_and_revocation_ends_it()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_an_existing_scoped_key_authenticates_realtime_as_that_key_and_revocation_ends_it().await
        }
        #[tokio::test]
        async fn an_owner_read_key_observes_its_thread_but_does_not_inherit_raw_protocol_management()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_an_owner_read_key_observes_its_thread_but_does_not_inherit_raw_protocol_management().await
        }
        #[tokio::test]
        async fn topic_sync_uses_the_same_selection_for_scoped_http_and_socket_child_rows()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::epoch_handoff_tests::__api_run_topic_sync_uses_the_same_selection_for_scoped_http_and_socket_child_rows().await
        }
    }
    #[cfg(test)]
    pub mod history_read_tests {
        pub use orch_api_base::realtime::history_read_tests::*;
        #[tokio::test]
        async fn document_changes_held_projection_rechecks_actual_sources_before_publication()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::history_read_tests::__api_run_document_changes_held_projection_rechecks_actual_sources_before_publication().await
        }
        #[tokio::test]
        async fn document_list_held_page_rechecks_its_actual_named_sources()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::history_read_tests::__api_run_document_list_held_page_rechecks_its_actual_named_sources().await
        }
        #[tokio::test]
        async fn legacy_document_reply_rechecks_held_sources_after_the_same_key_becomes_modern()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::history_read_tests::__api_run_legacy_document_reply_rechecks_held_sources_after_the_same_key_becomes_modern().await
        }
        #[tokio::test]
        async fn an_open_file_cannot_combine_old_scope_placement_with_new_ordinary_document_access()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::history_read_tests::__api_run_an_open_file_cannot_combine_old_scope_placement_with_new_ordinary_document_access().await
        }
        #[tokio::test]
        async fn scoped_history_segment_shrink_after_snapshot_refuses_stale_content()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::history_read_tests::__api_run_scoped_history_segment_shrink_after_snapshot_refuses_stale_content().await
        }
        #[tokio::test]
        async fn scoped_history_turn_shrink_after_snapshot_refuses_stale_content()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::history_read_tests::__api_run_scoped_history_turn_shrink_after_snapshot_refuses_stale_content().await
        }
        #[tokio::test]
        async fn http_sync_head_count_and_rows_stay_on_one_cut_across_a_terminal_commit()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::history_read_tests::__api_run_http_sync_head_count_and_rows_stay_on_one_cut_across_a_terminal_commit().await
        }
        #[tokio::test]
        async fn socket_rows_and_personalized_thread_do_not_mix_two_commits()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::history_read_tests::__api_run_socket_rows_and_personalized_thread_do_not_mix_two_commits().await
        }
        #[tokio::test]
        async fn a_late_wake_carries_every_changed_row_through_the_actual_snapshot_head()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::history_read_tests::__api_run_a_late_wake_carries_every_changed_row_through_the_actual_snapshot_head().await
        }
        #[tokio::test]
        async fn http_incremental_budget_and_rows_use_the_same_snapshot_as_its_head()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::history_read_tests::__api_run_http_incremental_budget_and_rows_use_the_same_snapshot_as_its_head().await
        }
        #[tokio::test]
        async fn opening_outline_and_conversation_keep_the_same_cut_while_the_native_turn_ends()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::history_read_tests::__api_run_opening_outline_and_conversation_keep_the_same_cut_while_the_native_turn_ends().await
        }
        #[tokio::test]
        async fn socket_request_cards_share_the_row_cut_and_never_share_another_readers_authority()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::history_read_tests::__api_run_socket_request_cards_share_the_row_cut_and_never_share_another_readers_authority().await
        }
        #[tokio::test]
        async fn scoped_raw_thread_output_rechecks_same_key_after_open_before_bytes()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::history_read_tests::__api_run_scoped_raw_thread_output_rechecks_same_key_after_open_before_bytes().await
        }
        #[tokio::test]
        async fn scoped_raw_thread_blob_rechecks_same_key_after_open_before_bytes()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::history_read_tests::__api_run_scoped_raw_thread_blob_rechecks_same_key_after_open_before_bytes().await
        }
        #[tokio::test]
        async fn browser_close_name_source_and_projection_never_mix_p_with_q()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::history_read_tests::__api_run_browser_close_name_source_and_projection_never_mix_p_with_q().await
        }
        #[tokio::test]
        async fn display_close_name_source_and_projection_never_mix_p_with_q()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::history_read_tests::__api_run_display_close_name_source_and_projection_never_mix_p_with_q().await
        }
    }
    #[cfg(test)]
    pub mod origin_read_tests {
        pub use orch_api_base::realtime::origin_read_tests::*;
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn browser_login_checks_its_actual_session_deadline_after_identity_wait()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::origin_read_tests::__api_run_browser_login_checks_its_actual_session_deadline_after_identity_wait().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn key_login_checks_its_actual_key_deadline_after_identity_wait()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::origin_read_tests::__api_run_key_login_checks_its_actual_key_deadline_after_identity_wait().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn real_socket_waits_capacity_before_detaching_a_projected_frame()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::origin_read_tests::__api_run_real_socket_waits_capacity_before_detaching_a_projected_frame().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn raw_session_expiry_cancels_an_unaccepted_real_socket_frame()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::origin_read_tests::__api_run_raw_session_expiry_cancels_an_unaccepted_real_socket_frame().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn reauthentication_after_subscription_check_retires_the_old_socket_projection()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::origin_read_tests::__api_run_reauthentication_after_subscription_check_retires_the_old_socket_projection().await
        }
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_rejected_resource_frame_keeps_its_same_row_replacement_available()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::origin_read_tests::__api_run_a_rejected_resource_frame_keeps_its_same_row_replacement_available().await
        }
    }

    pub mod presence {
        pub use orch_api_base::realtime::presence::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::realtime::presence::tests::*;
            #[test]
            fn a_person_is_listed_from_their_first_connection_until_their_last_one_leaves() {
                orch_api_base::realtime::presence::tests::__api_run_a_person_is_listed_from_their_first_connection_until_their_last_one_leaves()
            }
            #[test]
            fn a_connection_that_ends_while_showing_a_feed_takes_its_reader_with_it() {
                orch_api_base::realtime::presence::tests::__api_run_a_connection_that_ends_while_showing_a_feed_takes_its_reader_with_it()
            }
            #[test]
            fn what_other_processes_say_is_listed_with_what_this_one_knows_and_the_longest_reader_comes_first()
             {
                orch_api_base::realtime::presence::tests::__api_run_what_other_processes_say_is_listed_with_what_this_one_knows_and_the_longest_reader_comes_first()
            }
            #[test]
            fn a_process_that_stops_saying_it_is_there_is_forgotten_after_three_refreshes() {
                orch_api_base::realtime::presence::tests::__api_run_a_process_that_stops_saying_it_is_there_is_forgotten_after_three_refreshes()
            }
            #[test]
            fn what_this_process_knows_for_the_others_is_only_what_its_own_connections_show() {
                orch_api_base::realtime::presence::tests::__api_run_what_this_process_knows_for_the_others_is_only_what_its_own_connections_show()
            }
            #[test]
            fn a_list_is_sent_at_once_the_first_time_and_then_at_most_once_a_second() {
                orch_api_base::realtime::presence::tests::__api_run_a_list_is_sent_at_once_the_first_time_and_then_at_most_once_a_second()
            }
        }
    }

    pub mod resource_health {
        pub use orch_api_base::realtime::resource_health::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::realtime::resource_health::tests::*;
            #[test]
            fn unchanged_measures_send_no_new_status_just_because_time_passed() {
                orch_api_base::realtime::resource_health::tests::__api_run_unchanged_measures_send_no_new_status_just_because_time_passed()
            }
        }
    }

    pub mod rfb_filter {
        pub use orch_api_base::realtime::rfb_filter::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::realtime::rfb_filter::tests::*;
            #[test]
            fn keys_clicks_paste_resize_and_power_leave_the_next_picture_request_intact() {
                orch_api_base::realtime::rfb_filter::tests::__api_run_keys_clicks_paste_resize_and_power_leave_the_next_picture_request_intact()
            }
            #[test]
            fn picture_setup_continuous_updates_and_fences_pass_on_a_watching_stream() {
                orch_api_base::realtime::rfb_filter::tests::__api_run_picture_setup_continuous_updates_and_fences_pass_on_a_watching_stream()
            }
            #[test]
            fn unknown_and_oversized_messages_end_before_any_following_action_can_be_misread() {
                orch_api_base::realtime::rfb_filter::tests::__api_run_unknown_and_oversized_messages_end_before_any_following_action_can_be_misread()
            }
        }
    }

    pub mod terminals {
        pub use orch_api_base::realtime::terminals::*;
        #[cfg(test)]
        pub mod surface_tests {
            pub use orch_api_base::realtime::terminals::surface_tests::*;
            #[test]
            fn surface_terminal_ticket_keeps_its_original_door_until_matching_redemption() {
                orch_api_base::realtime::terminals::surface_tests::__api_run_surface_terminal_ticket_keeps_its_original_door_until_matching_redemption()
            }
        }
    }
    #[cfg(test)]
    pub mod ticket_scope_tests {
        pub use orch_api_base::realtime::ticket_scope_tests::*;

        pub mod terminal_write_socket_tests {
            pub use orch_api_base::realtime::ticket_scope_tests::terminal_write_socket_tests::*;
            #[tokio::test(flavor = "multi_thread")]
            async fn actual_full_terminal_pump_resumes_queued_marker_with_unchanged_authority()
            -> impl ::std::process::Termination {
                orch_api_base::realtime::ticket_scope_tests::terminal_write_socket_tests::__api_run_actual_full_terminal_pump_resumes_queued_marker_with_unchanged_authority().await
            }
            #[tokio::test(flavor = "multi_thread")]
            async fn actual_full_terminal_pump_stops_queued_marker_after_same_key_withdrawal()
            -> impl ::std::process::Termination {
                orch_api_base::realtime::ticket_scope_tests::terminal_write_socket_tests::__api_run_actual_full_terminal_pump_stops_queued_marker_after_same_key_withdrawal().await
            }
        }

        pub mod session_ticket_wire_tests {
            pub use orch_api_base::realtime::ticket_scope_tests::session_ticket_wire_tests::*;
            #[tokio::test(flavor = "multi_thread")]
            async fn actual_browser_control_keeps_own_read_but_stops_native_input_and_driving_after_manage_loss()
            -> impl ::std::process::Termination {
                orch_api_base::realtime::ticket_scope_tests::session_ticket_wire_tests::__api_run_actual_browser_control_keeps_own_read_but_stops_native_input_and_driving_after_manage_loss().await
            }
            #[tokio::test(flavor = "multi_thread")]
            async fn actual_display_control_keeps_own_read_but_stops_native_key_and_driving_after_manage_loss()
            -> impl ::std::process::Termination {
                orch_api_base::realtime::ticket_scope_tests::session_ticket_wire_tests::__api_run_actual_display_control_keeps_own_read_but_stops_native_key_and_driving_after_manage_loss().await
            }
            #[tokio::test(flavor = "multi_thread")]
            async fn actual_browser_current_read_loss_closes_before_new_native_tab_content()
            -> impl ::std::process::Termination {
                orch_api_base::realtime::ticket_scope_tests::session_ticket_wire_tests::__api_run_actual_browser_current_read_loss_closes_before_new_native_tab_content().await
            }
            #[tokio::test(flavor = "multi_thread")]
            async fn actual_display_current_read_loss_closes_before_new_native_pixel_bytes()
            -> impl ::std::process::Termination {
                orch_api_base::realtime::ticket_scope_tests::session_ticket_wire_tests::__api_run_actual_display_current_read_loss_closes_before_new_native_pixel_bytes().await
            }
            #[tokio::test(flavor = "multi_thread")]
            async fn actual_revoked_browser_ticket_never_runs_native_ensure_before_its_current_gate()
            -> impl ::std::process::Termination {
                orch_api_base::realtime::ticket_scope_tests::session_ticket_wire_tests::__api_run_actual_revoked_browser_ticket_never_runs_native_ensure_before_its_current_gate().await
            }
            #[tokio::test(flavor = "multi_thread")]
            async fn actual_display_rechecks_revoked_ticket_after_native_version_wait_before_first_bytes()
            -> impl ::std::process::Termination {
                orch_api_base::realtime::ticket_scope_tests::session_ticket_wire_tests::__api_run_actual_display_rechecks_revoked_ticket_after_native_version_wait_before_first_bytes().await
            }
            #[tokio::test(flavor = "multi_thread")]
            async fn an_actual_started_rfb_message_completes_only_its_own_tail_after_manage_loss()
            -> impl ::std::process::Termination {
                orch_api_base::realtime::ticket_scope_tests::session_ticket_wire_tests::__api_run_an_actual_started_rfb_message_completes_only_its_own_tail_after_manage_loss().await
            }
            #[tokio::test(flavor = "multi_thread")]
            async fn an_actual_zero_byte_queued_rfb_message_rechecks_manage_after_real_pending()
            -> impl ::std::process::Termination {
                orch_api_base::realtime::ticket_scope_tests::session_ticket_wire_tests::__api_run_an_actual_zero_byte_queued_rfb_message_rechecks_manage_after_real_pending().await
            }
        }
        #[tokio::test]
        async fn browser_view_ticket_redeem_rechecks_current_own_read()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::ticket_scope_tests::__api_run_browser_view_ticket_redeem_rechecks_current_own_read().await
        }
        #[tokio::test]
        async fn browser_control_ticket_keeps_own_read_but_drops_current_manage_after_narrowing()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::ticket_scope_tests::__api_run_browser_control_ticket_keeps_own_read_but_drops_current_manage_after_narrowing().await
        }
        #[tokio::test]
        async fn display_view_ticket_redeem_rechecks_current_own_read()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::ticket_scope_tests::__api_run_display_view_ticket_redeem_rechecks_current_own_read().await
        }
        #[tokio::test]
        async fn display_control_ticket_keeps_own_read_but_drops_current_manage_after_narrowing()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::ticket_scope_tests::__api_run_display_control_ticket_keeps_own_read_but_drops_current_manage_after_narrowing().await
        }
        #[tokio::test]
        async fn browser_view_only_ticket_never_escalates_into_input_after_scope_change()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::ticket_scope_tests::__api_run_browser_view_only_ticket_never_escalates_into_input_after_scope_change().await
        }
        #[tokio::test]
        async fn display_view_only_ticket_never_escalates_into_input_after_scope_change()
        -> impl ::std::process::Termination {
            orch_api_base::realtime::ticket_scope_tests::__api_run_display_view_only_ticket_never_escalates_into_input_after_scope_change().await
        }
    }

    pub mod user {
        pub use orch_api_base::realtime::user::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::realtime::user::tests::*;
            #[test]
            fn stored_file_metadata_is_removed_from_both_the_notice_and_its_action_copy() {
                orch_api_base::realtime::user::tests::__api_run_stored_file_metadata_is_removed_from_both_the_notice_and_its_action_copy()
            }
            #[test]
            fn a_notification_opens_the_path_its_kind_named_else_its_thread_else_the_approvals_else_its_place_else_the_inbox()
             {
                orch_api_base::realtime::user::tests::__api_run_a_notification_opens_the_path_its_kind_named_else_its_thread_else_the_approvals_else_its_place_else_the_inbox()
            }
            #[test]
            fn a_path_that_leaves_the_application_is_not_followed() {
                orch_api_base::realtime::user::tests::__api_run_a_path_that_leaves_the_application_is_not_followed()
            }
            #[test]
            fn a_row_of_a_kind_nobody_knows_is_no_notification() {
                orch_api_base::realtime::user::tests::__api_run_a_row_of_a_kind_nobody_knows_is_no_notification()
            }
            #[test]
            fn the_place_a_row_names_is_shown_as_the_reader_may_know_it() {
                orch_api_base::realtime::user::tests::__api_run_the_place_a_row_names_is_shown_as_the_reader_may_know_it()
            }
        }
    }

    pub mod watches {
        pub use orch_api_base::realtime::watches::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::realtime::watches::tests::*;
            #[test]
            fn the_processes_and_the_load_need_terminal_view_and_a_directory_files_view_on_the_environment()
             {
                orch_api_base::realtime::watches::tests::__api_run_the_processes_and_the_load_need_terminal_view_and_a_directory_files_view_on_the_environment()
            }
            #[test]
            fn no_other_feed_is_an_environments_to_decide_by_its_rows() {
                orch_api_base::realtime::watches::tests::__api_run_no_other_feed_is_an_environments_to_decide_by_its_rows()
            }
        }
    }
    #[cfg(test)]
    pub mod tests {
        pub use orch_api_base::realtime::tests::*;
        #[test]
        fn every_feed_with_a_sequence_has_a_source_and_the_users_own_feed_is_theirs_alone() {
            orch_api_base::realtime::tests::__api_run_every_feed_with_a_sequence_has_a_source_and_the_users_own_feed_is_theirs_alone()
        }
        #[test]
        fn a_feed_without_a_sequence_or_without_data_here_has_no_source() {
            orch_api_base::realtime::tests::__api_run_a_feed_without_a_sequence_or_without_data_here_has_no_source()
        }
        #[test]
        fn the_times_a_deployment_does_not_set_are_the_protocols_and_the_two_buffer_keys_are_the_deployments()
         {
            orch_api_base::realtime::tests::__api_run_the_times_a_deployment_does_not_set_are_the_protocols_and_the_two_buffer_keys_are_the_deployments()
        }
    }
}

pub mod registry {
    pub use orch_api_base::registry::*;
    #[cfg(test)]
    pub mod action_guard_state_tests {
        pub use orch_api_base::registry::action_guard_state_tests::*;
        #[tokio::test]
        async fn provisional_disclosure_and_operands_cannot_replace_final_or_opened_state() {
            orch_api_base::registry::action_guard_state_tests::__api_run_provisional_disclosure_and_operands_cannot_replace_final_or_opened_state().await
        }
        #[tokio::test]
        async fn current_user_checkpoint_cannot_replace_original_authentication_ceiling() {
            orch_api_base::registry::action_guard_state_tests::__api_run_current_user_checkpoint_cannot_replace_original_authentication_ceiling().await
        }
        #[tokio::test]
        async fn actual_http_identity_intersects_effective_original_and_current_admin_standing() {
            orch_api_base::registry::action_guard_state_tests::__api_run_actual_http_identity_intersects_effective_original_and_current_admin_standing().await
        }
    }
    #[cfg(test)]
    pub mod query_tests {
        pub use orch_api_base::registry::query_tests::*;
        #[test]
        fn storage_filters_and_flattened_paging_decode_through_the_real_query_adapter()
        -> impl ::std::process::Termination {
            orch_api_base::registry::query_tests::__api_run_storage_filters_and_flattened_paging_decode_through_the_real_query_adapter()
        }
    }
}

pub(crate) mod response_body {
    pub use orch_api_base::response_body::*;
    #[cfg(test)]
    #[allow(clippy::unwrap_used, reason = "transport controls fail by panicking")]
    pub mod tests {
        pub use orch_api_base::response_body::tests::*;
        #[tokio::test]
        async fn original_parts_data_trailers_and_upstream_errors_are_preserved() {
            orch_api_base::response_body::tests::__api_run_original_parts_data_trailers_and_upstream_errors_are_preserved().await
        }
        #[tokio::test]
        async fn current_check_occurs_after_an_awaited_upstream_frame() {
            orch_api_base::response_body::tests::__api_run_current_check_occurs_after_an_awaited_upstream_frame().await
        }
        #[tokio::test]
        async fn metadata_only_replies_are_checked_before_returning_their_headers() {
            orch_api_base::response_body::tests::__api_run_metadata_only_replies_are_checked_before_returning_their_headers().await
        }
    }
}

pub mod sign_in {
    pub use orch_api_base::sign_in::*;

    pub mod accounts {
        pub use orch_api_base::sign_in::accounts::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::sign_in::accounts::tests::*;
            #[test]
            fn a_username_is_lower_case_letters_digits_hyphens_and_underscores() {
                orch_api_base::sign_in::accounts::tests::__api_run_a_username_is_lower_case_letters_digits_hyphens_and_underscores()
            }
            #[test]
            fn what_a_person_gives_is_checked_all_at_once_and_tidied() {
                orch_api_base::sign_in::accounts::tests::__api_run_what_a_person_gives_is_checked_all_at_once_and_tidied()
            }
            #[test]
            fn a_new_person_starts_in_the_language_their_browser_asks_for_among_the_two_shipped() {
                orch_api_base::sign_in::accounts::tests::__api_run_a_new_person_starts_in_the_language_their_browser_asks_for_among_the_two_shipped()
            }
            #[test]
            fn an_address_has_something_before_the_at_and_a_dotted_name_after_it() {
                orch_api_base::sign_in::accounts::tests::__api_run_an_address_has_something_before_the_at_and_a_dotted_name_after_it()
            }
        }
    }

    pub mod codes {
        pub use orch_api_base::sign_in::codes::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::sign_in::codes::tests::*;
            #[test]
            fn a_mailed_code_is_eight_characters_that_cannot_be_mistaken_for_one_another() {
                orch_api_base::sign_in::codes::tests::__api_run_a_mailed_code_is_eight_characters_that_cannot_be_mistaken_for_one_another()
            }
            #[test]
            fn a_code_is_read_without_regard_to_case_hyphen_or_space() {
                orch_api_base::sign_in::codes::tests::__api_run_a_code_is_read_without_regard_to_case_hyphen_or_space()
            }
            #[test]
            fn only_the_hash_of_a_code_for_one_person_matches_it() {
                orch_api_base::sign_in::codes::tests::__api_run_only_the_hash_of_a_code_for_one_person_matches_it()
            }
            #[test]
            fn a_code_of_an_authenticator_is_six_digits_of_the_step_it_is_shown_in() {
                orch_api_base::sign_in::codes::tests::__api_run_a_code_of_an_authenticator_is_six_digits_of_the_step_it_is_shown_in()
            }
            #[test]
            fn a_colon_in_a_label_does_not_end_it() {
                orch_api_base::sign_in::codes::tests::__api_run_a_colon_in_a_label_does_not_end_it()
            }
            #[test]
            fn a_sealed_secret_opens_for_its_person_alone() {
                orch_api_base::sign_in::codes::tests::__api_run_a_sealed_secret_opens_for_its_person_alone()
            }
        }
    }

    pub mod mail {
        pub use orch_api_base::sign_in::mail::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::sign_in::mail::tests::*;
            #[test]
            fn a_link_is_the_public_origin_the_page_and_the_secret() {
                orch_api_base::sign_in::mail::tests::__api_run_a_link_is_the_public_origin_the_page_and_the_secret()
            }
            #[test]
            fn a_message_names_its_purpose_its_addressee_and_what_the_worker_needs() {
                orch_api_base::sign_in::mail::tests::__api_run_a_message_names_its_purpose_its_addressee_and_what_the_worker_needs()
            }
        }
    }

    pub mod passwords {
        pub use orch_api_base::sign_in::passwords::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::sign_in::passwords::tests::*;
            #[tokio::test]
            async fn a_password_is_kept_as_an_argon2id_hash_that_only_it_opens() {
                orch_api_base::sign_in::passwords::tests::__api_run_a_password_is_kept_as_an_argon2id_hash_that_only_it_opens().await
            }
            #[tokio::test]
            async fn no_hash_and_a_hash_that_does_not_read_are_never_right() {
                orch_api_base::sign_in::passwords::tests::__api_run_no_hash_and_a_hash_that_does_not_read_are_never_right().await
            }
            #[tokio::test]
            async fn a_name_nobody_has_is_checked_with_the_work_of_one_somebody_has() {
                orch_api_base::sign_in::passwords::tests::__api_run_a_name_nobody_has_is_checked_with_the_work_of_one_somebody_has().await
            }
            #[test]
            fn a_new_password_has_enough_characters_and_is_not_the_name() {
                orch_api_base::sign_in::passwords::tests::__api_run_a_new_password_has_enough_characters_and_is_not_the_name()
            }
            #[test]
            fn the_release_carries_the_pinned_list_of_common_passwords_and_the_value_it_adds() {
                orch_api_base::sign_in::passwords::tests::__api_run_the_release_carries_the_pinned_list_of_common_passwords_and_the_value_it_adds()
            }
            #[test]
            fn a_common_password_is_refused_whole_and_case_folded_and_a_phrase_that_holds_one_is_not()
             {
                orch_api_base::sign_in::passwords::tests::__api_run_a_common_password_is_refused_whole_and_case_folded_and_a_phrase_that_holds_one_is_not()
            }
        }
    }

    pub mod people {
        pub use orch_api_base::sign_in::people::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::sign_in::people::tests::*;
            #[test]
            fn an_avatar_shows_the_first_letter_as_a_person_reads_it_in_its_upper_case() {
                orch_api_base::sign_in::people::tests::__api_run_an_avatar_shows_the_first_letter_as_a_person_reads_it_in_its_upper_case()
            }
        }
    }

    pub mod proofs {
        pub use orch_api_base::sign_in::proofs::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::sign_in::proofs::tests::*;
            #[test]
            fn a_person_with_no_second_step_is_in_with_a_password_or_a_mailed_code_where_it_is_allowed()
             {
                orch_api_base::sign_in::proofs::tests::__api_run_a_person_with_no_second_step_is_in_with_a_password_or_a_mailed_code_where_it_is_allowed()
            }
            #[test]
            fn a_person_with_an_authenticator_needs_it_beside_the_password_or_a_mailed_code() {
                orch_api_base::sign_in::proofs::tests::__api_run_a_person_with_an_authenticator_needs_it_beside_the_password_or_a_mailed_code()
            }
            #[test]
            fn a_mailed_code_as_the_only_second_step_asks_for_the_password_after_a_mailed_code() {
                orch_api_base::sign_in::proofs::tests::__api_run_a_mailed_code_as_the_only_second_step_asks_for_the_password_after_a_mailed_code()
            }
            #[test]
            fn an_unused_recovery_code_replaces_the_app_but_never_the_first_factor() {
                orch_api_base::sign_in::proofs::tests::__api_run_an_unused_recovery_code_replaces_the_app_but_never_the_first_factor()
            }
            #[test]
            fn a_person_with_both_second_steps_may_finish_with_either_in_the_order_of_the_contract()
            {
                orch_api_base::sign_in::proofs::tests::__api_run_a_person_with_both_second_steps_may_finish_with_either_in_the_order_of_the_contract()
            }
        }
    }

    pub mod recovery_codes {
        pub use orch_api_base::sign_in::recovery_codes::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::sign_in::recovery_codes::tests::*;
            #[test]
            fn issued_codes_can_be_saved_and_typed_with_spaces_or_lower_case() {
                orch_api_base::sign_in::recovery_codes::tests::__api_run_issued_codes_can_be_saved_and_typed_with_spaces_or_lower_case()
            }
            #[test]
            fn a_saved_code_is_bound_to_its_person_and_is_not_a_mailed_code_hash() {
                orch_api_base::sign_in::recovery_codes::tests::__api_run_a_saved_code_is_bound_to_its_person_and_is_not_a_mailed_code_hash()
            }
        }
    }

    pub mod second_step {
        pub use orch_api_base::sign_in::second_step::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_base::sign_in::second_step::tests::*;
            #[test]
            fn a_requirement_counts_where_it_is_set_and_not_only_what_waits_for_it() {
                orch_api_base::sign_in::second_step::tests::__api_run_a_requirement_counts_where_it_is_set_and_not_only_what_waits_for_it()
            }
            #[test]
            fn last_seen_is_written_at_most_once_a_heartbeat() {
                orch_api_base::sign_in::second_step::tests::__api_run_last_seen_is_written_at_most_once_a_heartbeat()
            }
        }
    }
    #[cfg(test)]
    pub mod tests {
        pub use orch_api_base::sign_in::tests::*;
        #[test]
        fn the_limits_as_shipped_are_eight_a_name_in_three_hours_thirty_an_address_in_a_quarter_hour_and_five_codes_in_three_hours()
         {
            orch_api_base::sign_in::tests::__api_run_the_limits_as_shipped_are_eight_a_name_in_three_hours_thirty_an_address_in_a_quarter_hour_and_five_codes_in_three_hours()
        }
        #[test]
        fn a_cookie_carries_the_attributes_every_cookie_of_the_sign_in_has() {
            orch_api_base::sign_in::tests::__api_run_a_cookie_carries_the_attributes_every_cookie_of_the_sign_in_has()
        }
    }
}

pub mod syncs {
    pub use orch_api_base::syncs::*;
    #[cfg(test)]
    pub mod tests {
        pub use orch_api_base::syncs::tests::*;
        #[test]
        fn the_reply_is_made_of_the_members_and_the_numbers_it_ends_with() {
            orch_api_base::syncs::tests::__api_run_the_reply_is_made_of_the_members_and_the_numbers_it_ends_with()
        }
        #[test]
        fn one_member_that_found_too_much_makes_the_whole_incomplete() {
            orch_api_base::syncs::tests::__api_run_one_member_that_found_too_much_makes_the_whole_incomplete()
        }
        #[test]
        fn two_members_of_one_name_are_a_defect_and_not_an_answer() {
            orch_api_base::syncs::tests::__api_run_two_members_of_one_name_are_a_defect_and_not_an_answer()
        }
    }
}

pub mod thread_engine {
    pub use orch_api_base::thread_engine::*;
    #[cfg(test)]
    pub mod tests {
        pub use orch_api_base::thread_engine::tests::*;
        #[test]
        fn the_words_of_a_commands_row_are_the_stages_the_engine_knows() {
            orch_api_base::thread_engine::tests::__api_run_the_words_of_a_commands_row_are_the_stages_the_engine_knows()
        }
        #[test]
        fn a_failed_database_is_reported_as_the_databases_and_anything_else_sends_the_events_again()
        {
            orch_api_base::thread_engine::tests::__api_run_a_failed_database_is_reported_as_the_databases_and_anything_else_sends_the_events_again()
        }
        #[test]
        fn what_the_dispatcher_cannot_do_is_told_in_the_engines_words() {
            orch_api_base::thread_engine::tests::__api_run_what_the_dispatcher_cannot_do_is_told_in_the_engines_words()
        }
    }
}

pub mod tickets {
    pub use orch_api_base::tickets::*;
    #[cfg(test)]
    pub mod tests {
        pub use orch_api_base::tickets::tests::*;
        #[test]
        fn a_host_publication_verifies_as_its_named_node_and_keeps_the_opening_separate_from_its_ticket()
         {
            orch_api_base::tickets::tests::__api_run_a_host_publication_verifies_as_its_named_node_and_keeps_the_opening_separate_from_its_ticket()
        }
        #[test]
        fn a_request_in_a_stored_command_becomes_the_ticket_of_the_machine_it_goes_to() {
            orch_api_base::tickets::tests::__api_run_a_request_in_a_stored_command_becomes_the_ticket_of_the_machine_it_goes_to()
        }
        #[test]
        fn a_ticket_is_good_for_nobody_but_the_machine_it_was_signed_for() {
            orch_api_base::tickets::tests::__api_run_a_ticket_is_good_for_nobody_but_the_machine_it_was_signed_for()
        }
        #[test]
        fn a_tool_ticket_signs_one_environment_and_refuses_an_unscoped_request() {
            orch_api_base::tickets::tests::__api_run_a_tool_ticket_signs_one_environment_and_refuses_an_unscoped_request()
        }
        #[test]
        fn a_ticket_for_a_terminal_is_the_systems_own_and_short_lived() {
            orch_api_base::tickets::tests::__api_run_a_ticket_for_a_terminal_is_the_systems_own_and_short_lived()
        }
        #[test]
        fn a_request_that_cannot_be_a_ticket_says_what_is_wrong_with_it() {
            orch_api_base::tickets::tests::__api_run_a_request_that_cannot_be_a_ticket_says_what_is_wrong_with_it()
        }
    }
}

pub mod routes {
    pub use orch_api_base::routes::*;

    pub mod threads {
        pub use orch_api_threads::routes::threads::*;

        pub mod continuations {
            pub use orch_api_threads::routes::threads::continuations::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_threads::routes::threads::continuations::tests::*;
                #[test]
                fn an_admission_check_failure_retains_only_phase_and_safe_schema_identity() {
                    orch_api_threads::routes::threads::continuations::tests::__api_run_an_admission_check_failure_retains_only_phase_and_safe_schema_identity()
                }
                #[test]
                fn internal_diagnostics_do_not_format_row_values_or_arbitrary_error_text() {
                    orch_api_threads::routes::threads::continuations::tests::__api_run_internal_diagnostics_do_not_format_row_values_or_arbitrary_error_text()
                }
            }
        }

        pub mod model_inputs {
            pub use orch_api_threads::routes::threads::model_inputs::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_threads::routes::threads::model_inputs::tests::*;
                #[test]
                fn a_record_is_carried_whole_with_its_words_named() {
                    orch_api_threads::routes::threads::model_inputs::tests::__api_run_a_record_is_carried_whole_with_its_words_named()
                }
                #[tokio::test]
                async fn document_summary_inputs_and_their_blobs_need_the_documents_current_view_permission()
                 {
                    orch_api_threads::routes::threads::model_inputs::tests::__api_run_document_summary_inputs_and_their_blobs_need_the_documents_current_view_permission().await
                }
            }
        }

        pub mod parameters {
            pub use orch_api_threads::routes::threads::parameters::*;
            #[cfg(test)]
            #[allow(
                clippy::unwrap_used,
                reason = "malformed parameter fixtures fail the test"
            )]
            pub mod tests {
                pub use orch_api_threads::routes::threads::parameters::tests::*;
                #[test]
                fn capability_choices_follow_current_resolver_modes_without_rewriting_stale_choices()
                 {
                    orch_api_threads::routes::threads::parameters::tests::__api_run_capability_choices_follow_current_resolver_modes_without_rewriting_stale_choices()
                }
            }
        }

        pub mod view {
            pub use orch_api_threads::routes::threads::view::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_threads::routes::threads::view::tests::*;
                #[test]
                fn a_harness_with_a_memory_of_its_own_keeps_the_notes_beside_the_directory_and_one_with_none_leaves_them_to_the_agent()
                 {
                    orch_api_threads::routes::threads::view::tests::__api_run_a_harness_with_a_memory_of_its_own_keeps_the_notes_beside_the_directory_and_one_with_none_leaves_them_to_the_agent()
                }
                #[test]
                fn a_harness_that_runs_with_its_provider_cannot_keep_notes_in_the_directory() {
                    orch_api_threads::routes::threads::view::tests::__api_run_a_harness_that_runs_with_its_provider_cannot_keep_notes_in_the_directory()
                }
                #[test]
                fn the_setting_decides_whether_anyone_keeps_notes_at_all() {
                    orch_api_threads::routes::threads::view::tests::__api_run_the_setting_decides_whether_anyone_keeps_notes_at_all()
                }
            }
        }
    }

    pub mod documents {
        pub use orch_api_documents::routes::documents::*;

        pub mod references {
            pub use orch_api_documents::routes::documents::references::*;
            #[cfg(test)]
            pub mod scopes {
                pub use orch_api_documents::routes::documents::references::scopes::*;
                #[tokio::test]
                async fn reference_cards_bind_actual_attachment_owners_and_redact_unadmitted_parent_names()
                 {
                    orch_api_documents::routes::documents::references::scopes::__api_run_reference_cards_bind_actual_attachment_owners_and_redact_unadmitted_parent_names().await
                }
                #[tokio::test]
                async fn reference_cards_keep_each_actual_native_node_kind() {
                    orch_api_documents::routes::documents::references::scopes::__api_run_reference_cards_keep_each_actual_native_node_kind().await
                }
            }
            #[cfg(test)]
            pub mod source_waits {
                pub use orch_api_documents::routes::documents::references::source_waits::*;
                #[tokio::test]
                async fn actual_attachment_store_wait_never_substitutes_a_new_document_owner_generation_or_file()
                 {
                    orch_api_documents::routes::documents::references::source_waits::__api_run_actual_attachment_store_wait_never_substitutes_a_new_document_owner_generation_or_file().await
                }
                #[tokio::test]
                async fn actual_attachment_store_wait_keeps_the_initial_raw_access_deadline_even_if_storage_extends_it()
                 {
                    orch_api_documents::routes::documents::references::source_waits::__api_run_actual_attachment_store_wait_keeps_the_initial_raw_access_deadline_even_if_storage_extends_it().await
                }
                #[tokio::test]
                async fn actual_chat_attachment_wait_keeps_the_original_message_ordinal_and_material_digest()
                 {
                    orch_api_documents::routes::documents::references::source_waits::__api_run_actual_chat_attachment_wait_keeps_the_original_message_ordinal_and_material_digest().await
                }
            }
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_documents::routes::documents::references::tests::*;
                #[tokio::test]
                async fn file_cards_show_their_owner_size_and_lines_only_to_current_readers() {
                    orch_api_documents::routes::documents::references::tests::__api_run_file_cards_show_their_owner_size_and_lines_only_to_current_readers().await
                }
                #[tokio::test]
                async fn an_environment_file_card_uses_its_file_permission_and_can_show_a_kept_text_excerpt()
                 {
                    orch_api_documents::routes::documents::references::tests::__api_run_an_environment_file_card_uses_its_file_permission_and_can_show_a_kept_text_excerpt().await
                }
            }
        }

        pub mod text_creation {
            pub use orch_api_documents::routes::documents::text_creation::*;
            #[cfg(test)]
            pub mod replay_tests {
                pub use orch_api_documents::routes::documents::text_creation::replay_tests::*;
                #[tokio::test]
                async fn completed_creation_uses_new_credential_current_read_and_native_metadata_without_spent_sources()
                -> impl ::std::process::Termination {
                    orch_api_documents::routes::documents::text_creation::replay_tests::__api_run_completed_creation_uses_new_credential_current_read_and_native_metadata_without_spent_sources().await
                }
                #[tokio::test]
                async fn completed_creation_without_own_operation_read_returns_no_full_body_or_second_creation()
                -> impl ::std::process::Termination {
                    orch_api_documents::routes::documents::text_creation::replay_tests::__api_run_completed_creation_without_own_operation_read_returns_no_full_body_or_second_creation().await
                }
                #[tokio::test]
                async fn completed_creation_replay_is_bound_to_record_account_and_current_delivery_deadline()
                -> impl ::std::process::Termination {
                    orch_api_documents::routes::documents::text_creation::replay_tests::__api_run_completed_creation_replay_is_bound_to_record_account_and_current_delivery_deadline().await
                }
            }
        }
    }

    pub mod summaries {
        pub use orch_api_documents::routes::summaries::*;

        pub mod follow {
            pub use orch_api_documents::routes::summaries::follow::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_documents::routes::summaries::follow::tests::*;
                #[test]
                fn future_queue_waits_for_its_actual_due_time_and_eligible_work_uses_the_configured_pace()
                 {
                    orch_api_documents::routes::summaries::follow::tests::__api_run_future_queue_waits_for_its_actual_due_time_and_eligible_work_uses_the_configured_pace()
                }
            }
        }
    }

    pub mod resources {
        pub use orch_api_resources::routes::resources::*;

        pub mod agents {
            pub use orch_api_resources::routes::resources::agents::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_resources::routes::resources::agents::tests::*;
                #[test]
                fn a_builtin_adapter_requires_a_reported_allowed_program_but_a_self_contained_adapter_does_not()
                 {
                    orch_api_resources::routes::resources::agents::tests::__api_run_a_builtin_adapter_requires_a_reported_allowed_program_but_a_self_contained_adapter_does_not()
                }
                #[test]
                fn the_system_overrides_a_member_of_the_descriptor_and_says_which() {
                    orch_api_resources::routes::resources::agents::tests::__api_run_the_system_overrides_a_member_of_the_descriptor_and_says_which()
                }
                #[test]
                fn a_profile_has_four_members_and_the_same_home_choice_on_every_machine() {
                    orch_api_resources::routes::resources::agents::tests::__api_run_a_profile_has_four_members_and_the_same_home_choice_on_every_machine()
                }
                #[test]
                fn a_harness_kind_is_what_an_adapter_names_itself() {
                    orch_api_resources::routes::resources::agents::tests::__api_run_a_harness_kind_is_what_an_adapter_names_itself()
                }
            }
        }

        pub mod context {
            pub use orch_api_resources::routes::resources::context::*;
            #[cfg(test)]
            #[allow(
                clippy::unwrap_used,
                clippy::expect_used,
                reason = "a deciding case fails by panicking"
            )]
            pub mod tests {
                pub use orch_api_resources::routes::resources::context::tests::*;
                #[test]
                fn every_context_read_keeps_one_business_selection_and_both_real_independent_sources()
                 {
                    orch_api_resources::routes::resources::context::tests::__api_run_every_context_read_keeps_one_business_selection_and_both_real_independent_sources()
                }
                #[test]
                fn an_ordinary_home_or_name_fact_is_not_a_context_share() {
                    orch_api_resources::routes::resources::context::tests::__api_run_an_ordinary_home_or_name_fact_is_not_a_context_share()
                }
                #[test]
                fn the_actual_context_projection_never_serializes_home_owner_machine_profile_usage_or_other_work()
                 {
                    orch_api_resources::routes::resources::context::tests::__api_run_the_actual_context_projection_never_serializes_home_owner_machine_profile_usage_or_other_work()
                }
            }
        }

        pub mod hosts {
            pub use orch_api_resources::routes::resources::hosts::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_resources::routes::resources::hosts::tests::*;
                #[test]
                fn amounts_are_read_as_a_node_reports_them_or_as_the_api_names_them() {
                    orch_api_resources::routes::resources::hosts::tests::__api_run_amounts_are_read_as_a_node_reports_them_or_as_the_api_names_them()
                }
                #[test]
                fn what_a_node_reports_of_its_drivers_and_storages_is_shown_and_what_is_not_readable_is_left_out()
                 {
                    orch_api_resources::routes::resources::hosts::tests::__api_run_what_a_node_reports_of_its_drivers_and_storages_is_shown_and_what_is_not_readable_is_left_out()
                }
                #[test]
                fn labels_are_trimmed_kept_once_and_bounded() {
                    orch_api_resources::routes::resources::hosts::tests::__api_run_labels_are_trimmed_kept_once_and_bounded()
                }
            }
        }

        pub mod machines {
            pub use orch_api_resources::routes::resources::machines::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_resources::routes::resources::machines::tests::*;
                #[test]
                fn changed_driver_connections_report_every_schema_fault_without_echoing_values() {
                    orch_api_resources::routes::resources::machines::tests::__api_run_changed_driver_connections_report_every_schema_fault_without_echoing_values()
                }
                #[test]
                fn role_and_program_controls_follow_the_one_policy_before_the_next_machine_report()
                -> impl ::std::process::Termination {
                    orch_api_resources::routes::resources::machines::tests::__api_run_role_and_program_controls_follow_the_one_policy_before_the_next_machine_report()
                }
                #[test]
                fn example_or_missing_addresses_name_the_setting_and_prevent_minting_commands() {
                    orch_api_resources::routes::resources::machines::tests::__api_run_example_or_missing_addresses_name_the_setting_and_prevent_minting_commands()
                }
                #[test]
                fn enrollment_metadata_names_an_uninstallable_system_before_a_token_is_issued() {
                    orch_api_resources::routes::resources::machines::tests::__api_run_enrollment_metadata_names_an_uninstallable_system_before_a_token_is_issued()
                }
                #[test]
                fn enrollment_accepts_https_and_the_installers_loopback_http_exception() {
                    orch_api_resources::routes::resources::machines::tests::__api_run_enrollment_accepts_https_and_the_installers_loopback_http_exception()
                }
                #[test]
                fn minted_commands_follow_the_installers_shared_address_cases() {
                    orch_api_resources::routes::resources::machines::tests::__api_run_minted_commands_follow_the_installers_shared_address_cases()
                }
                #[test]
                fn the_commands_that_enroll_a_machine_carry_the_token_and_the_address_of_the_system()
                 {
                    orch_api_resources::routes::resources::machines::tests::__api_run_the_commands_that_enroll_a_machine_carry_the_token_and_the_address_of_the_system()
                }
                #[test]
                fn an_address_with_a_quote_or_a_shell_character_stays_one_word_in_every_shell() {
                    orch_api_resources::routes::resources::machines::tests::__api_run_an_address_with_a_quote_or_a_shell_character_stays_one_word_in_every_shell()
                }
                #[test]
                fn a_host_installer_uses_the_first_advertised_address_in_the_systems_order() {
                    orch_api_resources::routes::resources::machines::tests::__api_run_a_host_installer_uses_the_first_advertised_address_in_the_systems_order()
                }
                #[test]
                fn a_key_the_owner_handed_over_singly_or_all_at_once_may_be_changed_from_here_and_the_handing_over_never()
                 {
                    orch_api_resources::routes::resources::machines::tests::__api_run_a_key_the_owner_handed_over_singly_or_all_at_once_may_be_changed_from_here_and_the_handing_over_never()
                }
            }
        }

        pub mod mcp {
            pub use orch_api_resources::routes::resources::mcp::*;

            pub mod delete {
                pub use orch_api_resources::routes::resources::mcp::delete::*;
                #[cfg(test)]
                pub mod machine_tests {
                    pub use orch_api_resources::routes::resources::mcp::delete::machine_tests::*;
                    #[tokio::test]
                    async fn native_machine_deletion_uses_real_canonical_facets_and_queues_owned_custody_on_the_same_commit()
                    -> impl ::std::process::Termination {
                        orch_api_resources::routes::resources::mcp::delete::machine_tests::__api_run_native_machine_deletion_uses_real_canonical_facets_and_queues_owned_custody_on_the_same_commit().await
                    }
                    #[tokio::test]
                    async fn native_machine_deletion_refuses_presets_and_another_machines_canonical_import()
                    -> impl ::std::process::Termination {
                        orch_api_resources::routes::resources::mcp::delete::machine_tests::__api_run_native_machine_deletion_refuses_presets_and_another_machines_canonical_import().await
                    }
                    #[tokio::test]
                    async fn a_retained_ready_copy_does_not_let_its_machine_forget_another_machines_canonical_import()
                    -> impl ::std::process::Termination {
                        orch_api_resources::routes::resources::mcp::delete::machine_tests::__api_run_a_retained_ready_copy_does_not_let_its_machine_forget_another_machines_canonical_import().await
                    }
                    #[tokio::test]
                    async fn native_machine_deletion_reloads_current_owner_after_the_actual_authority_wait()
                    -> impl ::std::process::Termination {
                        orch_api_resources::routes::resources::mcp::delete::machine_tests::__api_run_native_machine_deletion_reloads_current_owner_after_the_actual_authority_wait().await
                    }
                }
            }
        }

        pub mod models {
            pub use orch_api_resources::routes::resources::models::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_resources::routes::resources::models::tests::*;
                #[test]
                fn a_routing_name_is_taken_as_it_is_given_and_a_display_name_is_trimmed() {
                    orch_api_resources::routes::resources::models::tests::__api_run_a_routing_name_is_taken_as_it_is_given_and_a_display_name_is_trimmed()
                }
                #[test]
                fn a_default_has_to_be_among_what_is_enabled_and_a_context_within_what_is_open() {
                    orch_api_resources::routes::resources::models::tests::__api_run_a_default_has_to_be_among_what_is_enabled_and_a_context_within_what_is_open()
                }
                #[test]
                fn reasoning_starts_require_enablement_and_suggestions_only_offer_them() {
                    orch_api_resources::routes::resources::models::tests::__api_run_reasoning_starts_require_enablement_and_suggestions_only_offer_them()
                }
                #[test]
                fn capacity_starts_require_enablement_and_suggestions_never_open_capacity() {
                    orch_api_resources::routes::resources::models::tests::__api_run_capacity_starts_require_enablement_and_suggestions_never_open_capacity()
                }
                #[test]
                fn accepting_a_cache_lifetime_suggestion_only_offers_the_additional_lifetime() {
                    orch_api_resources::routes::resources::models::tests::__api_run_accepting_a_cache_lifetime_suggestion_only_offers_the_additional_lifetime()
                }
                #[test]
                fn group_names_are_deduplicated_and_the_current_configured_count_is_independent() {
                    orch_api_resources::routes::resources::models::tests::__api_run_group_names_are_deduplicated_and_the_current_configured_count_is_independent()
                }
                #[test]
                fn a_larger_context_that_a_suggestion_lists_is_on_offer_and_no_start_comes_from_discovery()
                 {
                    orch_api_resources::routes::resources::models::tests::__api_run_a_larger_context_that_a_suggestion_lists_is_on_offer_and_no_start_comes_from_discovery()
                }
                #[test]
                fn accepting_a_report_without_enabled_options_preserves_its_future_facts() {
                    orch_api_resources::routes::resources::models::tests::__api_run_accepting_a_report_without_enabled_options_preserves_its_future_facts()
                }
                #[test]
                fn what_a_suggestion_lists_is_on_offer_and_none_of_it_is_enabled() {
                    orch_api_resources::routes::resources::models::tests::__api_run_what_a_suggestion_lists_is_on_offer_and_none_of_it_is_enabled()
                }
                #[test]
                fn search_offers_never_enable_the_tool_or_its_default() {
                    orch_api_resources::routes::resources::models::tests::__api_run_search_offers_never_enable_the_tool_or_its_default()
                }
            }
        }

        pub mod mounts {
            pub use orch_api_resources::routes::resources::mounts::*;

            pub mod creation {
                pub use orch_api_resources::routes::resources::mounts::creation::*;
                #[cfg(test)]
                pub mod replay_tests {
                    pub use orch_api_resources::routes::resources::mounts::creation::replay_tests::*;
                    #[tokio::test]
                    async fn singleton_completed_replay_uses_new_credential_native_current_style_and_spent_effects_are_absent()
                    -> impl ::std::process::Termination {
                        orch_api_resources::routes::resources::mounts::creation::replay_tests::__api_run_singleton_completed_replay_uses_new_credential_native_current_style_and_spent_effects_are_absent().await
                    }
                    #[tokio::test]
                    async fn singleton_completed_replay_keeps_receipts_account_boundary_and_current_publication_cut()
                    -> impl ::std::process::Termination {
                        orch_api_resources::routes::resources::mounts::creation::replay_tests::__api_run_singleton_completed_replay_keeps_receipts_account_boundary_and_current_publication_cut().await
                    }
                    #[tokio::test]
                    async fn singleton_auxiliary_read_uses_current_output_scope_after_native_locks_once()
                    -> impl ::std::process::Termination {
                        orch_api_resources::routes::resources::mounts::creation::replay_tests::__api_run_singleton_auxiliary_read_uses_current_output_scope_after_native_locks_once().await
                    }
                    #[tokio::test]
                    async fn singleton_auxiliary_read_keeps_first_sql_lifetime_and_does_not_run_without_response_read()
                    -> impl ::std::process::Termination {
                        orch_api_resources::routes::resources::mounts::creation::replay_tests::__api_run_singleton_auxiliary_read_keeps_first_sql_lifetime_and_does_not_run_without_response_read().await
                    }
                }
            }

            pub mod read {
                pub use orch_api_resources::routes::resources::mounts::read::*;
                #[cfg(test)]
                pub mod tests {
                    pub use orch_api_resources::routes::resources::mounts::read::tests::*;
                    #[tokio::test]
                    async fn snapshot_get_prepares_its_native_projection_once_for_each_http_credential()
                     {
                        orch_api_resources::routes::resources::mounts::read::tests::__api_run_snapshot_get_prepares_its_native_projection_once_for_each_http_credential().await
                    }
                    #[tokio::test]
                    async fn a_declared_snapshot_reader_cannot_return_content_without_its_actual_read_guard()
                     {
                        orch_api_resources::routes::resources::mounts::read::tests::__api_run_a_declared_snapshot_reader_cannot_return_content_without_its_actual_read_guard().await
                    }
                    #[tokio::test]
                    async fn actual_session_legacy_and_scoped_get_list_hold_native_bodies_and_errors_until_postcommit_deadline()
                     {
                        orch_api_resources::routes::resources::mounts::read::tests::__api_run_actual_session_legacy_and_scoped_get_list_hold_native_bodies_and_errors_until_postcommit_deadline().await
                    }
                    #[tokio::test]
                    async fn actual_native_get_auxiliary_hook_runs_once_outside_retry_and_keeps_errors_behind_the_final_cut()
                     {
                        orch_api_resources::routes::resources::mounts::read::tests::__api_run_actual_native_get_auxiliary_hook_runs_once_outside_retry_and_keeps_errors_behind_the_final_cut().await
                    }
                    #[tokio::test]
                    async fn native_get_context_uses_actual_session_mode_and_the_same_principal_time_after_opening()
                     {
                        orch_api_resources::routes::resources::mounts::read::tests::__api_run_native_get_context_uses_actual_session_mode_and_the_same_principal_time_after_opening().await
                    }
                }
            }

            pub mod volume {
                pub use orch_api_resources::routes::resources::mounts::volume::*;
                #[cfg(test)]
                pub mod tests {
                    pub use orch_api_resources::routes::resources::mounts::volume::tests::*;
                    #[tokio::test]
                    async fn native_volume_preparation_freezes_the_actual_driver_storage_and_owner_labels_without_issuing()
                     {
                        orch_api_resources::routes::resources::mounts::volume::tests::__api_run_native_volume_preparation_freezes_the_actual_driver_storage_and_owner_labels_without_issuing().await
                    }
                    #[tokio::test]
                    async fn native_volume_first_queue_cut_refuses_before_insert_and_positive_control_retains_one_exact_command()
                     {
                        orch_api_resources::routes::resources::mounts::volume::tests::__api_run_native_volume_first_queue_cut_refuses_before_insert_and_positive_control_retains_one_exact_command().await
                    }
                }
            }
        }

        pub mod projection {
            pub use orch_api_resources::routes::resources::projection::*;

            pub mod single {
                pub use orch_api_resources::routes::resources::projection::single::*;
                #[cfg(test)]
                pub mod tests {
                    pub use orch_api_resources::routes::resources::projection::single::tests::*;
                    #[tokio::test]
                    async fn document_cut_rejects_a_changed_hierarchy_after_snapshot_commit() {
                        orch_api_resources::routes::resources::projection::single::tests::__api_run_document_cut_rejects_a_changed_hierarchy_after_snapshot_commit().await
                    }
                    #[tokio::test]
                    async fn folder_cut_rejects_a_changed_membership_generation_after_snapshot_commit()
                     {
                        orch_api_resources::routes::resources::projection::single::tests::__api_run_folder_cut_rejects_a_changed_membership_generation_after_snapshot_commit().await
                    }
                    #[tokio::test]
                    async fn document_metadata_finisher_keeps_read_after_completed_manage_is_removed()
                     {
                        orch_api_resources::routes::resources::projection::single::tests::__api_run_document_metadata_finisher_keeps_read_after_completed_manage_is_removed().await
                    }
                    #[tokio::test]
                    async fn folder_metadata_finisher_keeps_read_after_completed_manage_is_removed()
                    {
                        orch_api_resources::routes::resources::projection::single::tests::__api_run_folder_metadata_finisher_keeps_read_after_completed_manage_is_removed().await
                    }
                    #[tokio::test]
                    async fn document_metadata_finisher_withholds_read_lost_after_commit() {
                        orch_api_resources::routes::resources::projection::single::tests::__api_run_document_metadata_finisher_withholds_read_lost_after_commit().await
                    }
                    #[tokio::test]
                    async fn folder_metadata_finisher_withholds_read_lost_after_commit() {
                        orch_api_resources::routes::resources::projection::single::tests::__api_run_folder_metadata_finisher_withholds_read_lost_after_commit().await
                    }
                    #[tokio::test]
                    async fn a_key_expiring_after_metadata_commit_keeps_only_its_accepted_receipt()
                    {
                        orch_api_resources::routes::resources::projection::single::tests::__api_run_a_key_expiring_after_metadata_commit_keeps_only_its_accepted_receipt().await
                    }
                }
            }
        }

        pub mod provider_login {
            pub use orch_api_resources::routes::resources::provider_login::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_resources::routes::resources::provider_login::tests::*;
                #[test]
                fn what_a_person_typed_is_read_as_the_type_of_the_field_it_was_typed_for() {
                    orch_api_resources::routes::resources::provider_login::tests::__api_run_what_a_person_typed_is_read_as_the_type_of_the_field_it_was_typed_for()
                }
                #[test]
                fn each_value_that_does_not_fit_its_field_is_named_by_the_field() {
                    orch_api_resources::routes::resources::provider_login::tests::__api_run_each_value_that_does_not_fit_its_field_is_named_by_the_field()
                }
                #[test]
                fn a_text_longer_than_a_field_takes_is_refused_and_a_secret_field_is_known_by_its_type()
                 {
                    orch_api_resources::routes::resources::provider_login::tests::__api_run_a_text_longer_than_a_field_takes_is_refused_and_a_secret_field_is_known_by_its_type()
                }
                #[test]
                fn a_sign_in_nobody_began_is_shown_as_none_running() {
                    orch_api_resources::routes::resources::provider_login::tests::__api_run_a_sign_in_nobody_began_is_shown_as_none_running()
                }
            }
        }

        pub mod provider_services {
            pub use orch_api_resources::routes::resources::provider_services::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_resources::routes::resources::provider_services::tests::*;
                #[test]
                fn editing_a_current_service_preserves_only_unchanged_stale_offerings_and_units()
                -> impl ::std::process::Termination {
                    orch_api_resources::routes::resources::provider_services::tests::__api_run_editing_a_current_service_preserves_only_unchanged_stale_offerings_and_units()
                }
            }
        }

        pub mod provider_sources {
            pub use orch_api_resources::routes::resources::provider_sources::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_resources::routes::resources::provider_sources::tests::*;
                #[test]
                fn only_a_registered_program_can_present_a_key_and_no_source_can_present_a_sign_in()
                {
                    orch_api_resources::routes::resources::provider_sources::tests::__api_run_only_a_registered_program_can_present_a_key_and_no_source_can_present_a_sign_in()
                }
                #[test]
                fn the_source_address_obeys_the_same_plain_http_policy_as_an_endpoint() {
                    orch_api_resources::routes::resources::provider_sources::tests::__api_run_the_source_address_obeys_the_same_plain_http_policy_as_an_endpoint()
                }
                #[test]
                fn a_sign_in_cannot_be_opted_into_an_address_and_a_program_from_another_machine_is_refused()
                 {
                    orch_api_resources::routes::resources::provider_sources::tests::__api_run_a_sign_in_cannot_be_opted_into_an_address_and_a_program_from_another_machine_is_refused()
                }
            }
        }

        pub mod providers {
            pub use orch_api_resources::routes::resources::providers::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_resources::routes::resources::providers::tests::*;
                #[test]
                fn a_provider_with_a_credential_and_a_model_is_ready_and_without_either_it_has_to_be_set_up()
                 {
                    orch_api_resources::routes::resources::providers::tests::__api_run_a_provider_with_a_credential_and_a_model_is_ready_and_without_either_it_has_to_be_set_up()
                }
                #[test]
                fn a_copy_of_the_credential_leaving_is_said_and_staying_home_is_not() {
                    orch_api_resources::routes::resources::providers::tests::__api_run_a_copy_of_the_credential_leaving_is_said_and_staying_home_is_not()
                }
                #[test]
                fn a_provider_allows_at_least_one_type_and_each_type_once() {
                    orch_api_resources::routes::resources::providers::tests::__api_run_a_provider_allows_at_least_one_type_and_each_type_once()
                }
                #[test]
                fn a_runtime_has_values_for_the_keys_of_its_own_scope_only() {
                    orch_api_resources::routes::resources::providers::tests::__api_run_a_runtime_has_values_for_the_keys_of_its_own_scope_only()
                }
                #[test]
                fn old_kept_credit_overrides_keep_their_precedence_as_balances_without_legacy_members_in_the_answer()
                 {
                    orch_api_resources::routes::resources::providers::tests::__api_run_old_kept_credit_overrides_keep_their_precedence_as_balances_without_legacy_members_in_the_answer()
                }
                #[test]
                fn manual_balance_and_spend_amounts_keep_decimal_precision_and_refuse_exponent_or_non_decimal_text()
                 {
                    orch_api_resources::routes::resources::providers::tests::__api_run_manual_balance_and_spend_amounts_keep_decimal_precision_and_refuse_exponent_or_non_decimal_text()
                }
            }
        }

        pub mod setup {
            pub use orch_api_resources::routes::resources::setup::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_resources::routes::resources::setup::tests::*;
                #[test]
                fn a_profile_that_has_reported_no_descriptor_declares_nothing_yet() {
                    orch_api_resources::routes::resources::setup::tests::__api_run_a_profile_that_has_reported_no_descriptor_declares_nothing_yet()
                }
                #[test]
                fn presets_are_the_vendors_endpoints_by_name_and_one_set_to_null_is_hidden() {
                    orch_api_resources::routes::resources::setup::tests::__api_run_presets_are_the_vendors_endpoints_by_name_and_one_set_to_null_is_hidden()
                }
                #[test]
                fn preset_key_hints_are_optional_public_metadata_and_keep_future_members() {
                    orch_api_resources::routes::resources::setup::tests::__api_run_preset_key_hints_are_optional_public_metadata_and_keep_future_members()
                }
                #[test]
                fn a_key_only_offers_the_types_in_which_it_stays_home_by_default() {
                    orch_api_resources::routes::resources::setup::tests::__api_run_a_key_only_offers_the_types_in_which_it_stays_home_by_default()
                }
                #[test]
                fn a_native_reader_without_a_proposal_uses_only_the_supported_creation_default() {
                    orch_api_resources::routes::resources::setup::tests::__api_run_a_native_reader_without_a_proposal_uses_only_the_supported_creation_default()
                }
            }
        }

        pub mod skills {
            pub use orch_api_resources::routes::resources::skills::*;

            pub mod draft_creation {
                pub use orch_api_resources::routes::resources::skills::draft_creation::*;
                #[cfg(test)]
                pub mod tests {
                    pub use orch_api_resources::routes::resources::skills::draft_creation::tests::*;
                    #[tokio::test]
                    async fn actual_new_skill_draft_keeps_current_http_deadline_after_blob_header_and_draft_file_waits()
                     {
                        orch_api_resources::routes::resources::skills::draft_creation::tests::__api_run_actual_new_skill_draft_keeps_current_http_deadline_after_blob_header_and_draft_file_waits().await
                    }
                }
            }

            pub mod source_url {
                pub use orch_api_resources::routes::resources::skills::source_url::*;
                #[cfg(test)]
                pub mod stage_tests {
                    pub use orch_api_resources::routes::resources::skills::source_url::stage_tests::*;
                    #[tokio::test]
                    async fn native_source_sealing_prepares_without_custody_and_consumes_one_real_first_insert_cut()
                     {
                        orch_api_resources::routes::resources::skills::source_url::stage_tests::__api_run_native_source_sealing_prepares_without_custody_and_consumes_one_real_first_insert_cut().await
                    }
                    #[tokio::test]
                    async fn native_source_public_noop_actor_and_transaction_rules_do_not_fabricate_secret_or_first_effect()
                     {
                        orch_api_resources::routes::resources::skills::source_url::stage_tests::__api_run_native_source_public_noop_actor_and_transaction_rules_do_not_fabricate_secret_or_first_effect().await
                    }
                }
                #[cfg(test)]
                pub mod tests {
                    pub use orch_api_resources::routes::resources::skills::source_url::tests::*;
                    #[test]
                    fn a_source_address_hides_userinfo_queries_and_fragments() {
                        orch_api_resources::routes::resources::skills::source_url::tests::__api_run_a_source_address_hides_userinfo_queries_and_fragments()
                    }
                }
            }
        }

        pub mod templates {
            pub use orch_api_resources::routes::resources::templates::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_resources::routes::resources::templates::tests::*;
                #[test]
                fn a_driver_is_named_like_a_program_and_a_spec_is_an_object() {
                    orch_api_resources::routes::resources::templates::tests::__api_run_a_driver_is_named_like_a_program_and_a_spec_is_an_object()
                }
            }
        }

        pub mod upstreams {
            pub use orch_api_resources::routes::resources::upstreams::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_resources::routes::resources::upstreams::tests::*;
                #[test]
                fn an_endpoint_on_the_runtimes_own_loopback_needs_no_tls_whatever_the_policy() {
                    orch_api_resources::routes::resources::upstreams::tests::__api_run_an_endpoint_on_the_runtimes_own_loopback_needs_no_tls_whatever_the_policy()
                }
                #[test]
                fn an_endpoint_is_https_and_an_upstream_has_one_for_each_protocol() {
                    orch_api_resources::routes::resources::upstreams::tests::__api_run_an_endpoint_is_https_and_an_upstream_has_one_for_each_protocol()
                }
                #[test]
                fn only_the_options_a_protocol_declares_are_taken_and_the_key_header_is_the_forwarders()
                 {
                    orch_api_resources::routes::resources::upstreams::tests::__api_run_only_the_options_a_protocol_declares_are_taken_and_the_key_header_is_the_forwarders()
                }
                #[test]
                fn an_upstream_that_takes_no_key_on_any_endpoint_is_keyless_and_secret_headers_are_collected()
                 {
                    orch_api_resources::routes::resources::upstreams::tests::__api_run_an_upstream_that_takes_no_key_on_any_endpoint_is_keyless_and_secret_headers_are_collected()
                }
                #[test]
                fn declared_model_defaults_survive_save_in_the_runtime_endpoint_shape() {
                    orch_api_resources::routes::resources::upstreams::tests::__api_run_declared_model_defaults_survive_save_in_the_runtime_endpoint_shape()
                }
                #[test]
                fn own_chat_output_limit_choices_survive_save_and_unknown_fields_are_refused() {
                    orch_api_resources::routes::resources::upstreams::tests::__api_run_own_chat_output_limit_choices_survive_save_and_unknown_fields_are_refused()
                }
                #[test]
                fn model_defaults_use_shared_object_and_owned_field_validation() {
                    orch_api_resources::routes::resources::upstreams::tests::__api_run_model_defaults_use_shared_object_and_owned_field_validation()
                }
            }
        }
    }

    pub mod environments {
        pub use orch_api_environments::routes::environments::*;

        pub mod agent_reach {
            pub use orch_api_environments::routes::environments::agent_reach::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_environments::routes::environments::agent_reach::tests::*;
                #[test]
                fn each_threads_line_names_its_arrangement_and_only_a_copy_has_an_expiry() {
                    orch_api_environments::routes::environments::agent_reach::tests::__api_run_each_threads_line_names_its_arrangement_and_only_a_copy_has_an_expiry()
                }
            }
        }

        pub mod exec_version {
            pub use orch_api_environments::routes::environments::exec_version::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_environments::routes::environments::exec_version::tests::*;
                #[test]
                fn a_manager_approves_only_the_reported_program_and_the_server_stamps_its_writer() {
                    orch_api_environments::routes::environments::exec_version::tests::__api_run_a_manager_approves_only_the_reported_program_and_the_server_stamps_its_writer()
                }
                #[test]
                fn a_missing_historical_writer_does_not_revoke_a_current_path_and_version() {
                    orch_api_environments::routes::environments::exec_version::tests::__api_run_a_missing_historical_writer_does_not_revoke_a_current_path_and_version()
                }
            }
        }

        pub mod incarnation {
            pub use orch_api_environments::routes::environments::incarnation::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_environments::routes::environments::incarnation::tests::*;
                #[test]
                fn source_members_and_deployment_key_change_the_opaque_value_without_revealing_inputs()
                -> impl ::std::process::Termination {
                    orch_api_environments::routes::environments::incarnation::tests::__api_run_source_members_and_deployment_key_change_the_opaque_value_without_revealing_inputs()
                }
                #[test]
                fn unrelated_metadata_and_home_changes_preserve_the_same_source_token()
                -> impl ::std::process::Termination {
                    orch_api_environments::routes::environments::incarnation::tests::__api_run_unrelated_metadata_and_home_changes_preserve_the_same_source_token()
                }
                #[test]
                fn an_absent_legacy_selection_remains_distinct_from_an_explicit_empty_selection()
                -> impl ::std::process::Termination {
                    orch_api_environments::routes::environments::incarnation::tests::__api_run_an_absent_legacy_selection_remains_distinct_from_an_explicit_empty_selection()
                }
            }
        }

        pub mod mcp {
            pub use orch_api_environments::routes::environments::mcp::*;

            pub mod import_sources {
                pub use orch_api_environments::routes::environments::mcp::import_sources::*;
                #[cfg(test)]
                pub mod error_tests {
                    pub use orch_api_environments::routes::environments::mcp::import_sources::error_tests::*;
                    #[test]
                    fn an_import_refusal_keeps_its_kind_without_exposing_native_details_as_an_offline_error()
                     {
                        orch_api_environments::routes::environments::mcp::import_sources::error_tests::__api_run_an_import_refusal_keeps_its_kind_without_exposing_native_details_as_an_offline_error()
                    }
                }
            }
        }

        pub mod network {
            pub use orch_api_environments::routes::environments::network::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_environments::routes::environments::network::tests::*;
                #[test]
                fn a_restricted_unit_is_never_shown_ready_before_its_host_said_so() {
                    orch_api_environments::routes::environments::network::tests::__api_run_a_restricted_unit_is_never_shown_ready_before_its_host_said_so()
                }
                #[test]
                fn an_old_open_unit_that_a_restriction_was_asked_of_is_kept_stopped_and_says_why() {
                    orch_api_environments::routes::environments::network::tests::__api_run_an_old_open_unit_that_a_restriction_was_asked_of_is_kept_stopped_and_says_why()
                }
                #[test]
                fn a_directory_on_a_persons_machine_has_the_machines_network() {
                    orch_api_environments::routes::environments::network::tests::__api_run_a_directory_on_a_persons_machine_has_the_machines_network()
                }
                #[test]
                fn a_thread_reader_sees_the_network_mode_and_state_without_its_private_addresses() {
                    orch_api_environments::routes::environments::network::tests::__api_run_a_thread_reader_sees_the_network_mode_and_state_without_its_private_addresses()
                }
            }
        }

        pub mod support {
            pub use orch_api_environments::routes::environments::support::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_environments::routes::environments::support::tests::*;
                #[test]
                fn an_environment_whose_facts_hold_nothing_of_it_was_found_to_have_nothing() {
                    orch_api_environments::routes::environments::support::tests::__api_run_an_environment_whose_facts_hold_nothing_of_it_was_found_to_have_nothing()
                }
                #[test]
                fn what_the_facts_hold_is_told_as_it_was_found() {
                    orch_api_environments::routes::environments::support::tests::__api_run_what_the_facts_hold_is_told_as_it_was_found()
                }
                #[test]
                fn one_odd_entry_does_not_hide_the_others() {
                    orch_api_environments::routes::environments::support::tests::__api_run_one_odd_entry_does_not_hide_the_others()
                }
            }
        }
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_environments::routes::environments::tests::*;
            #[test]
            fn existing_config_previews_validate_full_consent_and_patch_deferral_is_narrow() {
                orch_api_environments::routes::environments::tests::__api_run_existing_config_previews_validate_full_consent_and_patch_deferral_is_narrow()
            }
            #[test]
            fn a_root_is_an_absolute_path_of_the_machines_own_kind_that_does_not_climb() {
                orch_api_environments::routes::environments::tests::__api_run_a_root_is_an_absolute_path_of_the_machines_own_kind_that_does_not_climb()
            }
            #[test]
            fn a_root_must_lie_inside_a_root_the_machine_lists_and_a_machine_that_lists_none_is_asked()
             {
                orch_api_environments::routes::environments::tests::__api_run_a_root_must_lie_inside_a_root_the_machine_lists_and_a_machine_that_lists_none_is_asked()
            }
            #[test]
            fn what_the_exec_node_offers_is_what_is_configured_within_what_the_machine_offers() {
                orch_api_environments::routes::environments::tests::__api_run_what_the_exec_node_offers_is_what_is_configured_within_what_the_machine_offers()
            }
        }
    }

    pub mod workspace {
        pub use orch_api_environments::routes::workspace::*;

        pub mod files {
            pub use orch_api_environments::routes::workspace::files::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_environments::routes::workspace::files::tests::*;
                #[test]
                fn a_version_has_one_identifier_for_its_fetch_and_another_for_each_new_try() {
                    orch_api_environments::routes::workspace::files::tests::__api_run_a_version_has_one_identifier_for_its_fetch_and_another_for_each_new_try()
                }
                #[test]
                fn a_name_with_more_than_ascii_is_offered_to_a_download_in_both_forms() {
                    orch_api_environments::routes::workspace::files::tests::__api_run_a_name_with_more_than_ascii_is_offered_to_a_download_in_both_forms()
                }
                #[test]
                fn what_a_write_was_made_on_is_a_hash_or_the_word_none() {
                    orch_api_environments::routes::workspace::files::tests::__api_run_what_a_write_was_made_on_is_a_hash_or_the_word_none()
                }
            }
        }

        pub mod pages {
            pub use orch_api_environments::routes::workspace::pages::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_environments::routes::workspace::pages::tests::*;
                #[test]
                fn bounded_unsorted_selection_never_skips_a_large_omitted_sibling()
                -> impl ::std::process::Termination {
                    orch_api_environments::routes::workspace::pages::tests::__api_run_bounded_unsorted_selection_never_skips_a_large_omitted_sibling()
                }
                #[test]
                fn cursor_binding_and_expiry_fail_with_an_explicit_restart()
                -> impl ::std::process::Termination {
                    orch_api_environments::routes::workspace::pages::tests::__api_run_cursor_binding_and_expiry_fail_with_an_explicit_restart()
                }
                #[test]
                fn native_observation_interval_is_bound_into_the_http_cursor_and_reversed_intervals_are_refused()
                -> impl ::std::process::Termination {
                    orch_api_environments::routes::workspace::pages::tests::__api_run_native_observation_interval_is_bound_into_the_http_cursor_and_reversed_intervals_are_refused()
                }
            }
        }

        pub mod paths {
            pub use orch_api_environments::routes::workspace::paths::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_environments::routes::workspace::paths::tests::*;
                #[test]
                fn a_path_is_relative_and_does_not_climb() {
                    orch_api_environments::routes::workspace::paths::tests::__api_run_a_path_is_relative_and_does_not_climb()
                }
                #[test]
                fn a_path_has_a_parent_and_a_name() {
                    orch_api_environments::routes::workspace::paths::tests::__api_run_a_path_has_a_parent_and_a_name()
                }
            }
        }

        pub mod problems {
            pub use orch_api_environments::routes::workspace::problems::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_environments::routes::workspace::problems::tests::*;
                #[test]
                fn directory_capacity_refusal_is_available_to_retry_with_its_native_reason() {
                    orch_api_environments::routes::workspace::problems::tests::__api_run_directory_capacity_refusal_is_available_to_retry_with_its_native_reason()
                }
                #[test]
                fn the_same_refusal_of_a_machine_is_another_problem_for_each_thing_asked() {
                    orch_api_environments::routes::workspace::problems::tests::__api_run_the_same_refusal_of_a_machine_is_another_problem_for_each_thing_asked()
                }
            }
        }

        pub mod restores {
            pub use orch_api_environments::routes::workspace::restores::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_environments::routes::workspace::restores::tests::*;
                #[test]
                fn later_writer_names_follow_proved_paths_across_overlapping_directories() {
                    orch_api_environments::routes::workspace::restores::tests::__api_run_later_writer_names_follow_proved_paths_across_overlapping_directories()
                }
                #[test]
                fn later_changes_compare_added_removed_link_type_and_mode_without_guessing_unknowns()
                 {
                    orch_api_environments::routes::workspace::restores::tests::__api_run_later_changes_compare_added_removed_link_type_and_mode_without_guessing_unknowns()
                }
            }
        }
    }

    pub mod admin {
        pub use orch_api_admin::routes::admin::*;

        pub mod audit {
            pub use orch_api_admin::routes::admin::audit::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_admin::routes::admin::audit::tests::*;
                #[test]
                fn a_cell_that_a_spreadsheet_would_run_is_written_as_text_and_a_cell_with_a_comma_is_quoted()
                 {
                    orch_api_admin::routes::admin::audit::tests::__api_run_a_cell_that_a_spreadsheet_would_run_is_written_as_text_and_a_cell_with_a_comma_is_quoted()
                }
                #[test]
                fn a_file_is_named_by_its_span() {
                    orch_api_admin::routes::admin::audit::tests::__api_run_a_file_is_named_by_its_span()
                }
            }
        }

        pub mod catalogue {
            pub use orch_api_admin::routes::admin::catalogue::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_admin::routes::admin::catalogue::tests::*;
                #[tokio::test]
                async fn a_running_build_without_a_catalogue_check_keeps_its_declared_localized_name()
                 {
                    orch_api_admin::routes::admin::catalogue::tests::__api_run_a_running_build_without_a_catalogue_check_keeps_its_declared_localized_name().await
                }
                #[test]
                fn a_reported_version_has_no_borrowed_check_without_the_same_frozen_bytes() {
                    orch_api_admin::routes::admin::catalogue::tests::__api_run_a_reported_version_has_no_borrowed_check_without_the_same_frozen_bytes()
                }
                #[test]
                fn a_capability_key_is_what_the_schema_allows() {
                    orch_api_admin::routes::admin::catalogue::tests::__api_run_a_capability_key_is_what_the_schema_allows()
                }
            }
        }

        pub mod fleet {
            pub use orch_api_admin::routes::admin::fleet::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_admin::routes::admin::fleet::tests::*;
                #[test]
                fn a_span_gets_the_finest_step_that_gives_at_most_720_points() {
                    orch_api_admin::routes::admin::fleet::tests::__api_run_a_span_gets_the_finest_step_that_gives_at_most_720_points()
                }
                #[test]
                fn a_step_that_is_asked_for_must_be_one_of_the_four_and_fit() {
                    orch_api_admin::routes::admin::fleet::tests::__api_run_a_step_that_is_asked_for_must_be_one_of_the_four_and_fit()
                }
            }
        }

        pub mod identity_providers {
            pub use orch_api_admin::routes::admin::identity_providers::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_admin::routes::admin::identity_providers::tests::*;
                #[test]
                fn a_key_is_what_appears_in_a_callback_address() {
                    orch_api_admin::routes::admin::identity_providers::tests::__api_run_a_key_is_what_appears_in_a_callback_address()
                }
                #[test]
                fn scopes_must_include_openid_and_are_kept_once() {
                    orch_api_admin::routes::admin::identity_providers::tests::__api_run_scopes_must_include_openid_and_are_kept_once()
                }
                #[test]
                fn domains_are_lower_case_dotted_names_without_an_at_sign() {
                    orch_api_admin::routes::admin::identity_providers::tests::__api_run_domains_are_lower_case_dotted_names_without_an_at_sign()
                }
            }
        }

        pub mod orgs {
            pub use orch_api_admin::routes::admin::orgs::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_admin::routes::admin::orgs::tests::*;
                #[test]
                fn a_change_of_limits_is_told_in_the_words_of_what_it_does() {
                    orch_api_admin::routes::admin::orgs::tests::__api_run_a_change_of_limits_is_told_in_the_words_of_what_it_does()
                }
                #[test]
                fn a_cursor_of_orgs_keeps_a_name_with_any_characters() {
                    orch_api_admin::routes::admin::orgs::tests::__api_run_a_cursor_of_orgs_keeps_a_name_with_any_characters()
                }
            }
        }

        pub mod programs {
            pub use orch_api_admin::routes::admin::programs::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_admin::routes::admin::programs::tests::*;
                #[test]
                fn a_cursor_binds_the_list_search_program_and_filters() {
                    orch_api_admin::routes::admin::programs::tests::__api_run_a_cursor_binds_the_list_search_program_and_filters()
                }
                #[test]
                fn missing_declarations_are_not_reported_and_checks_cannot_turn_a_capability_back_on()
                 {
                    orch_api_admin::routes::admin::programs::tests::__api_run_missing_declarations_are_not_reported_and_checks_cannot_turn_a_capability_back_on()
                }
            }
        }

        pub mod users {
            pub use orch_api_admin::routes::admin::users::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_admin::routes::admin::users::tests::*;
                #[test]
                fn a_count_that_is_full_locks_until_its_oldest_needed_try_is_a_window_old() {
                    orch_api_admin::routes::admin::users::tests::__api_run_a_count_that_is_full_locks_until_its_oldest_needed_try_is_a_window_old()
                }
                #[test]
                fn a_username_is_what_the_schema_allows() {
                    orch_api_admin::routes::admin::users::tests::__api_run_a_username_is_what_the_schema_allows()
                }
            }
        }
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_admin::routes::admin::tests::*;
            #[test]
            fn an_address_is_http_or_https_with_a_host_and_no_space() {
                orch_api_admin::routes::admin::tests::__api_run_an_address_is_http_or_https_with_a_host_and_no_space()
            }
        }
    }

    pub mod auth {
        pub use orch_api_admin::routes::auth::*;

        pub mod oidc {
            pub use orch_api_admin::routes::auth::oidc::*;

            pub mod landing {
                pub use orch_api_admin::routes::auth::oidc::landing::*;
                #[cfg(test)]
                pub mod tests {
                    pub use orch_api_admin::routes::auth::oidc::landing::tests::*;
                    #[test]
                    fn a_long_base_is_cut_to_where_every_numbered_name_begins() {
                        orch_api_admin::routes::auth::oidc::landing::tests::__api_run_a_long_base_is_cut_to_where_every_numbered_name_begins()
                    }
                    #[test]
                    fn the_registration_mode_of_the_system_is_the_one_the_rules_read() {
                        orch_api_admin::routes::auth::oidc::landing::tests::__api_run_the_registration_mode_of_the_system_is_the_one_the_rules_read()
                    }
                }
            }
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_admin::routes::auth::oidc::tests::*;
                #[test]
                fn a_refusal_goes_where_the_sign_in_was_for() {
                    orch_api_admin::routes::auth::oidc::tests::__api_run_a_refusal_goes_where_the_sign_in_was_for()
                }
                #[test]
                fn the_mark_of_a_browser_is_sent_back_by_a_navigation_from_the_provider_and_only_here()
                 {
                    orch_api_admin::routes::auth::oidc::tests::__api_run_the_mark_of_a_browser_is_sent_back_by_a_navigation_from_the_provider_and_only_here()
                }
                #[test]
                fn a_proof_lasts_five_minutes() {
                    orch_api_admin::routes::auth::oidc::tests::__api_run_a_proof_lasts_five_minutes(
                    )
                }
            }
        }

        pub mod token_activity {
            pub use orch_api_admin::routes::auth::token_activity::*;
            #[cfg(test)]
            #[allow(clippy::expect_used, reason = "a test fails by panicking")]
            pub mod tests {
                pub use orch_api_admin::routes::auth::token_activity::tests::*;
                #[test]
                fn history_cursors_are_bound_to_the_actual_key_and_org_slice() {
                    orch_api_admin::routes::auth::token_activity::tests::__api_run_history_cursors_are_bound_to_the_actual_key_and_org_slice()
                }
            }
        }

        pub mod tokens {
            pub use orch_api_admin::routes::auth::tokens::*;
            #[cfg(test)]
            #[allow(clippy::unwrap_used, reason = "a boundary control fails by panicking")]
            pub mod tests {
                pub use orch_api_admin::routes::auth::tokens::tests::*;
                #[test]
                fn omitted_modes_create_explicit_deny_all_and_explicit_legacy_fields_stay_distinct()
                {
                    orch_api_admin::routes::auth::tokens::tests::__api_run_omitted_modes_create_explicit_deny_all_and_explicit_legacy_fields_stay_distinct()
                }
                #[test]
                fn issuance_rejects_unknown_future_selections_before_storage() {
                    orch_api_admin::routes::auth::tokens::tests::__api_run_issuance_rejects_unknown_future_selections_before_storage()
                }
                #[test]
                fn key_issuance_requires_complete_current_admission_and_cannot_issue_a_legacy_escape()
                 {
                    orch_api_admin::routes::auth::tokens::tests::__api_run_key_issuance_requires_complete_current_admission_and_cannot_issue_a_legacy_escape()
                }
            }
        }
    }

    pub mod personal {
        pub use orch_api_admin::routes::personal::*;

        pub mod viewed {
            pub use orch_api_admin::routes::personal::viewed::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_admin::routes::personal::viewed::tests::*;
                #[test]
                fn only_the_served_complete_page_can_be_recorded() {
                    orch_api_admin::routes::personal::viewed::tests::__api_run_only_the_served_complete_page_can_be_recorded()
                }
            }
        }
    }

    pub mod settings {
        pub use orch_api_admin::routes::settings::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_admin::routes::settings::tests::*;
            #[test]
            fn zero_disables_these_features_without_losing_their_units() {
                orch_api_admin::routes::settings::tests::__api_run_zero_disables_these_features_without_losing_their_units()
            }
        }
    }

    pub mod users {
        pub use orch_api_admin::routes::users::*;

        pub mod invitation_terms {
            pub use orch_api_admin::routes::users::invitation_terms::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_admin::routes::users::invitation_terms::tests::*;
                #[test]
                fn the_public_hash_is_sha256_of_the_exact_resolved_utf8_text() {
                    orch_api_admin::routes::users::invitation_terms::tests::__api_run_the_public_hash_is_sha256_of_the_exact_resolved_utf8_text()
                }
            }
        }
    }

    pub mod access {
        pub use orch_api_collaboration::routes::access::*;

        pub mod page_operations {
            pub use orch_api_collaboration::routes::access::page_operations::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_collaboration::routes::access::page_operations::tests::*;
                #[tokio::test]
                async fn refusal_keeps_opaque_trusted_or_untrusted_error_and_changes_no_page_audit_or_native_dto()
                 {
                    orch_api_collaboration::routes::access::page_operations::tests::__api_run_refusal_keeps_opaque_trusted_or_untrusted_error_and_changes_no_page_audit_or_native_dto().await
                }
                #[tokio::test]
                async fn genuine_noop_never_calls_boundary_and_multirow_private_change_calls_only_once()
                 {
                    orch_api_collaboration::routes::access::page_operations::tests::__api_run_genuine_noop_never_calls_boundary_and_multirow_private_change_calls_only_once().await
                }
                #[tokio::test]
                async fn an_empty_owners_object_cannot_relabel_a_whole_page_change_as_an_ownership_operation()
                 {
                    orch_api_collaboration::routes::access::page_operations::tests::__api_run_an_empty_owners_object_cannot_relabel_a_whole_page_change_as_an_ownership_operation().await
                }
            }
        }
    }

    pub mod nodes {
        pub use orch_api_collaboration::routes::nodes::*;

        pub mod tree_work {
            pub use orch_api_collaboration::routes::nodes::tree_work::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_collaboration::routes::nodes::tree_work::tests::*;
                #[test]
                fn a_readers_approval_is_not_replaced_by_someone_elses_question() {
                    orch_api_collaboration::routes::nodes::tree_work::tests::__api_run_a_readers_approval_is_not_replaced_by_someone_elses_question()
                }
                #[test]
                fn only_a_known_current_binding_refusal_names_its_setup_cause() {
                    orch_api_collaboration::routes::nodes::tree_work::tests::__api_run_only_a_known_current_binding_refusal_names_its_setup_cause()
                }
            }
        }
    }

    pub mod notifications {
        pub use orch_api_collaboration::routes::notifications::*;

        pub mod views {
            pub use orch_api_collaboration::routes::notifications::views::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_collaboration::routes::notifications::views::tests::*;
                #[test]
                fn a_member_sees_a_channel_without_its_fields_its_secrets_its_failures_and_its_count()
                 {
                    orch_api_collaboration::routes::notifications::views::tests::__api_run_a_member_sees_a_channel_without_its_fields_its_secrets_its_failures_and_its_count()
                }
                #[test]
                fn a_route_nobody_changed_is_off_for_everything_and_at_once() {
                    orch_api_collaboration::routes::notifications::views::tests::__api_run_a_route_nobody_changed_is_off_for_everything_and_at_once()
                }
                #[test]
                fn the_texts_of_a_channel_are_read_back_as_they_were_kept() {
                    orch_api_collaboration::routes::notifications::views::tests::__api_run_the_texts_of_a_channel_are_read_back_as_they_were_kept()
                }
            }
        }
    }

    pub mod search {
        pub use orch_api_collaboration::routes::search::*;

        pub mod content {
            pub use orch_api_collaboration::routes::search::content::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_collaboration::routes::search::content::tests::*;
                #[test]
                fn excerpts_and_highlights_use_characters_for_any_script_and_lowercase_expansions()
                {
                    orch_api_collaboration::routes::search::content::tests::__api_run_excerpts_and_highlights_use_characters_for_any_script_and_lowercase_expansions()
                }
            }
        }
    }

    pub mod transfer {
        pub use orch_api_collaboration::routes::transfer::*;
        #[cfg(test)]
        pub mod tests {
            pub use orch_api_collaboration::routes::transfer::tests::*;
            #[test]
            fn transfer_kind_refusals_name_the_kinds_the_request_answers_for() {
                orch_api_collaboration::routes::transfer::tests::__api_run_transfer_kind_refusals_name_the_kinds_the_request_answers_for()
            }
        }
    }

    pub mod usage {
        pub use orch_api_interactive::routes::usage::*;

        pub mod csv {
            pub use orch_api_interactive::routes::usage::csv::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_interactive::routes::usage::csv::tests::*;
                #[test]
                fn native_text_cannot_become_a_spreadsheet_formula_even_behind_controls() {
                    orch_api_interactive::routes::usage::csv::tests::__api_run_native_text_cannot_become_a_spreadsheet_formula_even_behind_controls()
                }
                #[test]
                fn statistics_keep_exact_numeric_bytes_and_ordinary_native_words() {
                    orch_api_interactive::routes::usage::csv::tests::__api_run_statistics_keep_exact_numeric_bytes_and_ordinary_native_words()
                }
                #[test]
                fn a_cell_with_a_comma_a_quote_or_a_line_break_is_quoted_and_its_quotes_are_doubled()
                 {
                    orch_api_interactive::routes::usage::csv::tests::__api_run_a_cell_with_a_comma_a_quote_or_a_line_break_is_quoted_and_its_quotes_are_doubled()
                }
            }
        }

        pub mod figures {
            pub use orch_api_interactive::routes::usage::figures::*;
            #[cfg(test)]
            pub mod tests {
                pub use orch_api_interactive::routes::usage::figures::tests::*;
                #[test]
                fn degradation_requires_ten_requests_and_twenty_percent_errors_or_more_than_triple_latency()
                 {
                    orch_api_interactive::routes::usage::figures::tests::__api_run_degradation_requires_ten_requests_and_twenty_percent_errors_or_more_than_triple_latency()
                }
                #[test]
                fn a_span_is_cut_by_the_hour_up_to_two_days_by_the_day_to_92_by_the_week_to_two_years_and_then_by_the_month()
                 {
                    orch_api_interactive::routes::usage::figures::tests::__api_run_a_span_is_cut_by_the_hour_up_to_two_days_by_the_day_to_92_by_the_week_to_two_years_and_then_by_the_month()
                }
                #[test]
                fn every_bucket_of_a_span_has_a_point_and_a_week_begins_on_the_monday_before_the_span()
                 {
                    orch_api_interactive::routes::usage::figures::tests::__api_run_every_bucket_of_a_span_has_a_point_and_a_week_begins_on_the_monday_before_the_span()
                }
                #[test]
                fn credits_are_written_in_their_shortest_exact_form() {
                    orch_api_interactive::routes::usage::figures::tests::__api_run_credits_are_written_in_their_shortest_exact_form()
                }
            }
        }
    }

    pub mod approvals {
        pub use orch_api_threads::routes::approvals::*;
    }

    pub mod forks {
        pub use orch_api_threads::routes::forks::*;
    }

    pub mod links {
        pub use orch_api_threads::routes::links::*;
    }

    pub mod share {
        pub use orch_api_threads::routes::share::*;
    }

    pub mod topics {
        pub use orch_api_threads::routes::topics::*;
    }

    pub mod document_tree {
        pub use orch_api_documents::routes::document_tree::*;
    }

    pub mod downloads {
        pub use orch_api_documents::routes::downloads::*;
    }

    pub mod exports {
        pub use orch_api_documents::routes::exports::*;
    }

    pub mod imports {
        pub use orch_api_documents::routes::imports::*;
    }

    pub mod storage {
        pub use orch_api_documents::routes::storage::*;
    }

    pub mod capabilities {
        pub use orch_api_resources::routes::capabilities::*;
    }

    pub mod duplicates {
        pub use orch_api_resources::routes::duplicates::*;
    }

    pub mod trust {
        pub use orch_api_resources::routes::trust::*;
    }

    pub mod fleet {
        pub use orch_api_environments::routes::fleet::*;
    }

    pub mod loss {
        pub use orch_api_environments::routes::loss::*;
    }

    pub mod processes {
        pub use orch_api_environments::routes::processes::*;
    }

    pub mod admin_messages {
        pub use orch_api_admin::routes::admin_messages::*;
    }

    pub mod directory {
        pub use orch_api_admin::routes::directory::*;
    }

    pub mod first_steps {
        pub use orch_api_admin::routes::first_steps::*;
    }

    pub mod profiles {
        pub use orch_api_admin::routes::profiles::*;
    }

    pub mod chats {
        pub use orch_api_collaboration::routes::chats::*;
    }

    pub mod destinations {
        pub use orch_api_collaboration::routes::destinations::*;
    }

    pub mod everything {
        pub use orch_api_collaboration::routes::everything::*;
    }

    pub mod trace {
        pub use orch_api_collaboration::routes::trace::*;
    }

    pub mod browsers {
        pub use orch_api_interactive::routes::browsers::*;
    }

    pub mod displays {
        pub use orch_api_interactive::routes::displays::*;
    }

    pub mod gateways {
        pub use orch_api_interactive::routes::gateways::*;
    }

    pub mod interactive_close {
        pub use orch_api_interactive::routes::interactive_close::*;
    }

    pub mod interactive_creation {
        pub use orch_api_interactive::routes::interactive_creation::*;
    }

    pub mod interactive_tickets {
        pub use orch_api_interactive::routes::interactive_tickets::*;
    }

    pub mod meta {
        pub use orch_api_interactive::routes::meta::*;
    }

    pub mod port_entry {
        pub use orch_api_interactive::routes::port_entry::*;
    }

    pub mod port_requests {
        pub use orch_api_interactive::routes::port_requests::*;
    }

    pub mod ports {
        pub use orch_api_interactive::routes::ports::*;
    }

    pub mod terminals {
        pub use orch_api_interactive::routes::terminals::*;
    }
    pub fn mount(registry: &mut orch_api_base::registry::Registry) {
        orch_api_base::route_assembly::initialize();
        orch_api_base::routes::mount(registry);
    }
}
#[cfg(test)]
pub(crate) mod desk {
    pub use orch_api_base::desk::*;
}
#[cfg(test)]
pub(crate) mod native_close_support {
    pub use orch_api_base::native_close_support::*;
}
#[cfg(test)]
pub(crate) mod places {
    pub use orch_api_base::places::*;
}
#[cfg(test)]
pub(crate) mod realtime_support {
    pub use orch_api_base::realtime_support::*;
}
#[cfg(test)]
pub(crate) mod rig {
    pub use orch_api_base::rig::*;
}
#[cfg(test)]
pub(crate) mod session_ticket_support {
    pub use orch_api_base::session_ticket_support::*;
}
#[cfg(test)]
pub(crate) mod support {
    pub use orch_api_base::support::*;
}
#[cfg(test)]
pub(crate) mod thread_history_scope {
    pub use orch_api_base::thread_history_scope::*;
}

pub mod component_updates {
    pub use orch_api_base::component_updates::*;
}

pub mod harness_checks {
    pub use orch_api_base::harness_checks::*;
}

pub mod machine_requests {
    pub use orch_api_base::machine_requests::*;
}

pub mod openapi {
    pub use orch_api_base::openapi::*;
}

pub mod recordings {
    pub use orch_api_base::recordings::*;
}

pub(crate) mod sandbox_readiness {
    pub use orch_api_base::sandbox_readiness::*;
}

pub mod server {
    pub use orch_api_base::server::*;
}

pub mod state {
    pub use orch_api_base::state::*;
}

pub mod testing {
    pub use orch_api_base::testing::*;
}

pub mod thread_access {
    pub use orch_api_base::thread_access::*;
}

pub mod thread_markers {
    pub use orch_api_base::thread_markers::*;
}

pub mod thread_read_tools {
    pub use orch_api_base::thread_read_tools::*;
}

pub(crate) mod upload_inventory {
    pub use orch_api_base::upload_inventory::*;
}

pub mod uploads {
    pub use orch_api_base::uploads::*;
}
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

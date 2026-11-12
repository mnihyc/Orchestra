//! Hook observations share the ordinary harness marker and expose no prompt, path or output body.
use super::*;

#[derive(Debug)]
pub(super) struct HookRun {
    native_turn: Option<String>,
    turn: Option<Id>,
    name: Option<String>,
    status: String,
    duration_ms: Option<i64>,
}

impl Mapper {
    pub(super) fn hook_updated(
        &mut self,
        method: &str,
        params: &Json,
        out: &mut Vec<SessionEvent>,
        ctx: &Context<'_>,
    ) {
        let run = params.get("run").unwrap_or(&Json::Null);
        let status = if method == "hook/started" {
            "running"
        } else {
            text(run, "status")
        };
        self.observe_hook(
            params,
            text(run, "id"),
            opt_text(run, "eventName"),
            status,
            run.get("durationMs")
                .and_then(Json::as_i64)
                .filter(|duration| *duration >= 0),
            false,
            out,
            ctx,
        );
    }

    pub(super) fn hook_prompt(
        &mut self,
        params: &Json,
        out: &mut Vec<SessionEvent>,
        ctx: &Context<'_>,
    ) {
        for fragment in params
            .pointer("/item/fragments")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            self.observe_hook(
                params,
                text(fragment, "hookRunId"),
                None,
                "completed",
                None,
                true,
                out,
                ctx,
            );
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "native hook metadata remains grouped at its single projection"
    )]
    fn observe_hook(
        &mut self,
        params: &Json,
        id: &str,
        name: Option<String>,
        status: &str,
        duration: Option<i64>,
        prompt: bool,
        out: &mut Vec<SessionEvent>,
        ctx: &Context<'_>,
    ) {
        if id.is_empty()
            || !matches!(
                status,
                "running" | "completed" | "failed" | "blocked" | "stopped"
            )
        {
            return;
        }
        let thread = text(params, "threadId");
        let Some(stream) = self.stream_of(thread) else {
            return;
        };
        let key = (thread.to_owned(), id.to_owned());
        let native = params
            .get("turnId")
            .and_then(Json::as_str)
            .map(str::to_owned);
        let known = self.hook_runs.get(&key);
        let turn = if let Some(known) = known {
            if !prompt && known.native_turn != native {
                return;
            }
            known.turn
        } else if let Some(native) = &native {
            if thread == self.config.native_thread {
                let Some(turn) = self.turn.as_ref().filter(|turn| &turn.native == native) else {
                    return;
                };
                Some(turn.id)
            } else {
                let Some(child) = self
                    .children
                    .get(thread)
                    .filter(|child| child.native_turn.as_ref() == Some(native))
                else {
                    return;
                };
                child.turn_id
            }
        } else {
            None
        };
        let name = name
            .filter(|name| !name.trim().is_empty())
            .or_else(|| known.and_then(|known| known.name.clone()));
        let status = known
            .filter(|known| known.status != "running" && (prompt || status == "running"))
            .map_or_else(|| status.to_owned(), |known| known.status.clone());
        let duration = duration.or_else(|| known.and_then(|known| known.duration_ms));
        let Some((_, mut data)) = self.native_settings.state(
            thread,
            "hook",
            Some(id.to_owned()),
            json!({"status":status,"event_name":name,"duration_ms":duration}),
            turn,
            ctx.clock.time(),
        ) else {
            return;
        };
        let native_turn = known.map_or(native, |known| known.native_turn.clone());
        data.subject = name.clone();
        data.changed = true;
        // Managed launches disable environment and configured hooks alike.
        // A reported execution is a factual pin violation, never permission to run it.
        data.expected = orch_api_contract::usage::UsageMeasurement::Told {
            value: json!(false),
        };
        data.diverged = true;
        self.hook_runs.insert(
            key,
            HookRun {
                native_turn,
                turn,
                name,
                status,
                duration_ms: duration,
            },
        );
        let parent = self
            .children
            .get(thread)
            .and_then(|child| child.parent_item.clone());
        let native_ref = format!(
            "hook-{}",
            Hash::of(json!([thread, id]).to_string().as_bytes())
        );
        self.emit_harness_observation(native_ref, &stream, parent, data, None, out);
    }
}

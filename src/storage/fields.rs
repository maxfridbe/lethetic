//! Static mappings from native/browser transport names to shared SQL columns.
pub(super) const NATIVE_SESSION: &[&str] = &[
    "latest_logical_turn_id=accounting.latest_logical_turn_id:s",
    "session_id=session_id:s",
    "display_name=display_name:s",
    "schema_version=schema_version:n",
    "system_prompt=system_prompt:s",
    "theme=theme_name:s",
    "connection_id=connection_id:s",
    "model_name=model_name:s",
    "hide_thinking=hide_thinking:b",
    "loop_mode=loop_mode:s",
    "remote_control=remote_control:s",
    "remote_control_open=remote_control_open:b",
    "remote_control_files=remote_control_files:b",
    "python_runtime_id=python_runtime_id:s",
    "model_ms=session_times.engine_ms:n",
    "tool_ms=session_times.tool_ms:n",
    "elapsed_ms=session_times.idle_ms:n",
];
pub(super) const BROWSER_SESSION: &[&str] = &[
    "context_tokens=metrics.context.tokens:n",
    "context_source=metrics.context.source:s",
    "session_id=id:s",
    "display_name=name:s",
    "label=label:s",
    "updated_at=updatedAt:s",
    "system_prompt=system:s",
    "theme=theme:s",
    "mode=mode:s",
    "python=python:b",
    "network_access=networkAccess:b",
    "nuget_access=nugetAccess:b",
    "site_access=siteAccess:b",
    "relay_endpoint=dotnetRelayEndpoint:s",
    "loop_mode=loopDetection:s",
    "model_ms=metrics.modelMs:n",
    "tool_ms=metrics.toolMs:n",
    "elapsed_ms=metrics.elapsedMs:n",
];
pub(super) const CONNECTION: &[&str] = &[
    "name=name:s",
    "provider=provider:s",
    "endpoint=endpoint:s",
    "model=model:s",
    "thinking=thinking:s",
];
pub(super) const MESSAGE: &[&str] = &[
    "role=role:s",
    "content=content:s",
    "tool_result_is_error=tool_result_is_error:b",
    "tool_call_id=tool_call_id:s",
];
pub(super) const CALL: &[&str] = &[
    "call_id=id:s",
    "provider_id=provider_id:s",
    "function_name=function.name:s",
    "arguments_json=function.arguments:j",
    "call_type=type:s",
];
pub(super) const PART: &[&str] = &["part_type=type:s", "text=text:s"];
pub(super) const BLOCK: &[&str] = &[
    "content=content:s",
    "title=title:s",
    "success=success:b",
    "logical_turn_id=logical_turn_id:s",
    "duration_ms=duration_ms:n",
    "duration_label=duration_label:s",
    "prompt_tokens=prompt_tokens:n",
    "completion_tokens=completion_tokens:n",
    "tool_name=tool.tool_name:s",
    "tool_kind=tool.kind:s",
    "tool_payload=tool.payload:s",
];
pub(super) const USAGE: &[&str] = &[
    "uncached_input_tokens=usage.uncached_input_tokens:n",
    "cache_read_input_tokens=usage.cache_read_input_tokens:n",
    "cache_creation_input_tokens=usage.cache_creation_input_tokens:n",
    "output_tokens=usage.output_tokens:n",
    "total_input_tokens=usage.total_input_tokens:n",
    "total_tokens=usage.total_tokens:n",
    "breakdown_complete=usage.breakdown_complete:b",
    "reported_cost_nanos=usage.reported_cost_nanos:n",
];
pub(super) const COST: &[&str] = &[
    "cost_currency=estimated_cost.currency:s",
    "cost_nanos=estimated_cost.nanos:n",
    "cost_incomplete=estimated_cost.incomplete:b",
    "cost_mixed_pricing=estimated_cost.mixed_pricing:b",
    "cost_long_context=estimated_cost.long_context_applied:b",
    "cost_effective_as_of=estimated_cost.pricing_effective_as_of:s",
    "cost_valid_through=estimated_cost.pricing_valid_through:s",
    "cost_provenance=estimated_cost.provenance_kind:s",
    "cost_display=estimated_cost.display:s",
];
pub(super) const REQUEST: &[&str] = &[
    "request_id=request_id:s",
    "logical_turn_id=logical_turn_id:s",
    "connection_id=connection_id:s",
    "model=model:s",
    "completed=completed:b",
    "in_flight=in_flight:b",
];
pub(super) const NATIVE_TOTAL: &[&str] = &[
    "request_count=request_count:n",
    "long_context_request_count=long_context_request_count:n",
    "unpriced_request_count=unpriced_request_count:n",
    "incomplete_usage_request_count=incomplete_usage_request_count:n",
];
pub(super) const BROWSER_TOTAL: &[&str] = &[
    "input_reports=reports.input:n",
    "output_reports=reports.output:n",
    "total_reports=reports.total:n",
    "cached_reports=reports.cached:n",
    "cache_write_reports=reports.cacheWrite:n",
    "reasoning_reports=reports.reasoning:n",
    "request_count=requests:n",
    "responses=responses:n",
    "input_tokens=sums.input:n",
    "output_tokens=sums.output:n",
    "total_tokens=sums.total:n",
    "cached_tokens=sums.cached:n",
    "cache_write_tokens=sums.cacheWrite:n",
    "reasoning_tokens=sums.reasoning:n",
];
pub(super) const FILE: &[&str] = &["path=path:s", "modified_at=modifiedAt:n"];
pub(super) const TODO: &[&str] = &[
    "todo_id=id:s",
    "content=content:s",
    "status=status:s",
    "priority=priority:s",
];
pub(super) fn block(browser: bool) -> Vec<&'static str> {
    [
        if browser {
            &["kind=kind:s"][..]
        } else {
            &["kind=block_type:s"][..]
        },
        BLOCK,
        USAGE,
        COST,
    ]
    .concat()
}
pub(super) fn request() -> Vec<&'static str> {
    [REQUEST, USAGE, COST].concat()
}
pub(super) fn total(browser: bool) -> Vec<&'static str> {
    if browser {
        BROWSER_TOTAL.to_vec()
    } else {
        [NATIVE_TOTAL, USAGE, COST].concat()
    }
}

pub(super) const BROWSER_LATEST: &[&str] = &[
    "input_tokens=input:n",
    "output_tokens=output:n",
    "total_tokens=total:n",
    "cached_tokens=cached:n",
    "cache_write_tokens=cacheWrite:n",
    "reasoning_tokens=reasoning:n",
];

pub(super) const PROVIDER_TRACE: &[&str] = &[
    "trace_id=id:s",
    "turn_id=turnId:s",
    "round=round:n",
    "attempt=attempt:n",
    "provider=provider:s",
    "model=model:s",
    "started_at=startedAt:s",
    "ended_at=endedAt:s",
    "outcome=outcome:s",
    "http_status=httpStatus:n",
    "content_type=contentType:s",
    "request_body=requestBody:s",
    "request_bytes=requestBytes:n",
    "request_omitted_bytes=requestOmittedBytes:n",
    "response_head=responseHead:s",
    "response_tail=responseTail:s",
    "received_bytes=receivedBytes:n",
    "response_omitted_bytes=responseOmittedBytes:n",
    "response_complete=responseComplete:b",
    "redacted=redacted:b",
    "error=error:s",
];

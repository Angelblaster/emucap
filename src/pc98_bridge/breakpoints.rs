use super::*;

impl<G: GdbTransport> Bridge<G> {
    pub(super) fn set_breakpoint(&mut self, params: &Value) -> BridgeResult<Value> {
        let kind = params
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("exec")
            .to_string();
        let zkind = match kind.as_str() {
            "exec" => "0",
            "write" => "2",
            "read" => "3",
            "access" => "4",
            _ => {
                return Err(BridgeError::BadParams(
                    "MAME PC-98 supports exec/read/write/access breakpoints".into(),
                ))
            }
        };
        let memory_type = params
            .get("memory_type")
            .and_then(Value::as_str)
            .unwrap_or("physical");
        let region = memory_region(memory_type).ok_or_else(|| {
            BridgeError::BadParams(format!("unsupported memory_type: {memory_type}"))
        })?;
        let start = required_num(params, "start")?;
        // `end`는 포함(inclusive) region 오프셋이며, 없으면 start 한 단위다. start보다 작으면 1바이트로 접는다.
        let end = optional_num(params, "end")?.unwrap_or(start).max(start);
        let size = end - start + 1;
        // [start, start+size)가 선택된 region 안이어야 한다 — 유한 region(ram·tvram 등) 밖 offset을
        // region.base로 감싸 MAME setpoint에 넘기면 절대 안 맞을 BP가 조용히 서므로 거부한다
        // (nds_bridge route()의 범위 가드와 동형).
        if !matches!(start.checked_add(size), Some(last) if last <= region.size as u64) {
            return Err(BridgeError::BadParams(format!(
                "{memory_type} breakpoint out of range: offset {start:#x}+{size:#x} exceeds region size {region_size:#x}",
                region_size = region.size
            )));
        }
        let addr = region.base as u64 + start;
        let snapshots = parse_snapshot_specs(params.get("snapshot"))?;
        let condition = breakpoint_condition(params, &kind)?;
        let pause_on_hit = params
            .get("pause_on_hit")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let spec = format!(
            "{zkind}|{addr:x}|{size:x}|{}|{condition}",
            if pause_on_hit { 1 } else { 0 }
        );
        let resp = self.lua_cmd_reply("setpoint", Some(&spec))?;
        let (backend, backend_id) = parse_breakpoint_reply(&resp)?;
        let id = self.next_bp;
        self.next_bp += 1;
        self.bps.insert(
            id,
            Breakpoint {
                kind,
                addr: Some(addr),
                size: Some(size),
                backend,
                backend_id,
                condition: empty_to_none(condition),
                snapshots,
                pause_on_hit,
                register: None,
                state_key: None,
                min: None,
                max: None,
            },
        );
        Ok(json!({ "id": id }))
    }

    pub(super) fn watch_register(&mut self, params: &Value) -> BridgeResult<Value> {
        let raw_reg = params
            .get("register")
            .and_then(Value::as_str)
            .unwrap_or("sp");
        let (expr_reg, state_key) = normalize_debug_register(raw_reg)?;
        let min = optional_num(params, "min")?.unwrap_or(0);
        let max = optional_num(params, "max")?.unwrap_or(0xFFFF_FFFF);
        if min > max {
            return Err(BridgeError::BadParams("min must be <= max".into()));
        }
        let condition = format!("({expr_reg} < {min:X}) || ({expr_reg} > {max:X})");
        let pause_on_hit = params
            .get("pause_on_hit")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let spec = format!("{}|{condition}", if pause_on_hit { 1 } else { 0 });
        let resp = self.lua_cmd_reply("setregpoint", Some(&spec))?;
        let backend_id = parse_regpoint_reply(&resp)?;
        let id = self.next_bp;
        self.next_bp += 1;
        self.bps.insert(
            id,
            Breakpoint {
                kind: "reg".into(),
                addr: None,
                size: None,
                backend: "rp".into(),
                backend_id,
                condition: Some(condition),
                snapshots: Vec::new(),
                pause_on_hit,
                register: Some(raw_reg.to_string()),
                state_key: Some(state_key.into()),
                min: Some(min),
                max: Some(max),
            },
        );
        Ok(json!({ "id": id }))
    }

    pub(super) fn clear_breakpoint(&mut self, params: &Value) -> BridgeResult<Value> {
        let id = required_num(params, "id")?;
        let bp = self
            .bps
            .get(&id)
            .cloned()
            .ok_or_else(|| BridgeError::BadParams(format!("unknown breakpoint id: {id}")))?;
        let spec = format!("{}|{}", bp.backend, bp.backend_id);
        let resp = self.lua_cmd_raw("clearpoint", Some(&spec))?;
        if resp != "OK" && resp != "E00" {
            return Err(BridgeError::Emulator(format!(
                "MAME breakpoint clear failed: {resp}"
            )));
        }
        self.bps.remove(&id);
        Ok(json!({ "cleared": id }))
    }

    pub(super) fn list_breakpoints(&self) -> BridgeResult<Value> {
        let mut rows = Vec::new();
        for (id, bp) in &self.bps {
            if bp.kind == "reg" {
                rows.push(json!({
                    "id": id,
                    "kind": "reg",
                    "register": bp.register.clone(),
                    "min": bp.min,
                    "max": bp.max,
                    "condition": bp.condition.clone(),
                }));
            } else {
                let start = bp.addr.unwrap_or(0);
                let size = bp.size.unwrap_or(1);
                rows.push(json!({
                    "id": id,
                    "kind": bp.kind.clone(),
                    "start": start,
                    "end": start + size.saturating_sub(1),
                    "condition": bp.condition.clone(),
                }));
            }
        }
        Ok(json!({ "breakpoints": rows }))
    }

    pub(super) fn clear_all_breakpoints(&mut self) -> BridgeResult<Value> {
        let mut cleared = Vec::new();
        for id in self.bps.keys().copied().collect::<Vec<_>>() {
            if self.clear_breakpoint(&json!({ "id": id })).is_ok() {
                cleared.push(id);
            }
        }
        Ok(json!({ "cleared": cleared }))
    }

    pub(super) fn poll_events(&mut self, params: &Value) -> BridgeResult<Value> {
        self.drain_stop()?;
        let saw_reset = self.drain_reset_event()?;
        let filter_id = optional_num(params, "breakpoint_id")?;
        let mut events = Vec::new();
        let mut remaining = Vec::new();
        for mut event in std::mem::take(&mut self.events) {
            if saw_reset
                && event.get("type").and_then(Value::as_str) == Some("stop")
                && event.get("raw").and_then(Value::as_str) == Some("S05")
            {
                continue;
            }
            self.enrich_event(&mut event);
            if let Some(obj) = event.as_object_mut() {
                obj.remove("_pc98_enriched");
            }
            if let Some(filter_id) = filter_id {
                if event.get("id").and_then(Value::as_u64) != Some(filter_id) {
                    remaining.push(event);
                    continue;
                }
            }
            events.push(event);
        }
        self.events = remaining;
        Ok(json!({ "events": events, "dropped": 0 }))
    }
}

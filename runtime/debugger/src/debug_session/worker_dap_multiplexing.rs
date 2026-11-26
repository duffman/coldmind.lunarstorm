//! Custom multiplexing for independent worker DAP sessions. Each nested message
//! keeps its own DAP sequence/reference namespace; the root only adds an envelope.
use super::*;
use std::sync::Arc;

pub(super) struct TargetSessions {
    broker: Arc<DebugTargetBroker>,
    root: DebugTargetId,
    children: Vec<(DebugTargetId, DapAdapter)>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TargetArgs {
    target_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TargetRequestArgs {
    target_id: String,
    request: Box<RawValue>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StopTargetArgs {
    target_id: String,
    #[serde(default)]
    subtree: bool,
}

impl DapAdapter {
    /// Construct the root DAP session with bounded worker discovery and routing.
    ///
    /// `jetbunny/targets` lists broker-issued string IDs. `jetbunny/attachTarget`
    /// claims a child controller once. `jetbunny/targetRequest` carries that ID
    /// and one ordinary DAP request; child responses and idle events arrive in
    /// `jetbunny/targetMessage` envelopes. This custom protocol requires an IDE
    /// integration; it does not advertise standard DAP child-session support.
    /// Root EOF, disconnect, failure, or drop detaches all child controllers and
    /// disables future debugger admission without terminating the application.
    pub fn with_targets(
        handle: VmDebugHandle,
        broker: Arc<DebugTargetBroker>,
        root: DebugTargetId,
    ) -> Result<Self, DapError> {
        if !broker
            .list()?
            .iter()
            .any(|target| target.id == root && !target.exited)
        {
            return Err(DebugError::InvalidReference.into());
        }
        let mut children = Vec::new();
        children
            .try_reserve_exact(broker.limit())
            .map_err(|_| DapError::Capacity)?;
        let mut adapter = Self::new(handle)?;
        adapter.targets = Some(TargetSessions {
            broker,
            root,
            children,
        });
        Ok(adapter)
    }

    pub(super) fn detach_target_sessions(&mut self) {
        if let Some(mut targets) = self.targets.take() {
            // Disable admission first: a worker racing disconnect cannot leave
            // another queued break-on-start controller behind after this drain.
            let _ = targets.broker.detach_debuggers();
            for (_, child) in &mut targets.children {
                let _ = child.end_input();
            }
        }
    }

    pub(super) fn target_request(
        &mut self,
        request: &Request,
        output: &mut Vec<Vec<u8>>,
    ) -> Result<(), DapError> {
        let mut targets = self.targets.take().ok_or(DebugError::Unsupported)?;
        let result = self.target_request_inner(&mut targets, request, output);
        self.targets = Some(targets);
        result
    }

    fn target_request_inner(
        &mut self,
        targets: &mut TargetSessions,
        request: &Request,
        output: &mut Vec<Vec<u8>>,
    ) -> Result<(), DapError> {
        let body = match request.command.as_str() {
            "jetbunny/targets" => {
                let mut entries = Vec::new();
                let metadata = targets.broker.list()?;
                entries
                    .try_reserve_exact(metadata.len())
                    .map_err(|_| DapError::Capacity)?;
                for target in metadata {
                    let mut fields = vec![
                        ("id", text(&target.id.wire_id())),
                        ("name", Value::Text(target.name)),
                        ("exited", boolean(target.exited)),
                        (
                            "attached",
                            boolean(
                                target.id == targets.root
                                    || targets.children.iter().any(|(id, _)| *id == target.id),
                            ),
                        ),
                    ];
                    if let Some(parent) = target.parent {
                        fields.push(("parent", text(&parent.wire_id())));
                    }
                    entries.push(obj(fields));
                }
                obj(vec![
                    ("rootId", text(&targets.root.wire_id())),
                    ("targets", array(entries)),
                ])
            }
            "jetbunny/attachTarget" => {
                let arguments: TargetArgs = args(request)?;
                let id = targets.broker.resolve_wire_id(&arguments.target_id)?;
                if id == targets.root || targets.children.iter().any(|(child, _)| *child == id) {
                    return Err(DebugError::InFlight.into());
                }
                if targets.children.len() >= targets.broker.limit() {
                    return Err(DapError::Capacity);
                }
                let handle = targets.broker.take_debugger(id)?;
                // A running child has no entry barrier to claim or synthesize.
                // Configuration only controls subsequent debugger commands.
                let child = Self::new_session(handle, false)?;
                targets.children.push((id, child));
                obj(vec![("targetId", text(&id.wire_id()))])
            }
            "jetbunny/targetRequest" => {
                let arguments: TargetRequestArgs = args(request)?;
                let id = targets.broker.resolve_wire_id(&arguments.target_id)?;
                let index = targets
                    .children
                    .iter()
                    .position(|(child, _)| *child == id)
                    .ok_or(DebugError::NotAttached)?;
                let result = {
                    let child = &mut targets.children[index].1;
                    let input = DapFramer::new(child.framer.max_input, child.framer.max_input)?
                        .encode(arguments.request.get().as_bytes())?;
                    child.receive(&input)
                };
                match result.and_then(|frames| self.wrap_target_frames(id, frames)) {
                    Ok(frames) => output.extend(frames),
                    Err(error) => {
                        let (_, mut child) = targets.children.remove(index);
                        let _ = child.end_input();
                        output.push(self.target_closed(id, &error)?);
                        return Err(error);
                    }
                }
                if targets.children[index].1.is_disconnected() {
                    targets.children.remove(index);
                }
                obj(vec![("targetId", text(&id.wire_id()))])
            }
            "jetbunny/stopTarget" => {
                let arguments: StopTargetArgs = args(request)?;
                let id = targets.broker.resolve_wire_id(&arguments.target_id)?;
                if arguments.subtree {
                    targets.broker.stop_subtree(id)?;
                } else {
                    targets.broker.request_stop(id)?;
                }
                obj(vec![("targetId", text(&id.wire_id()))])
            }
            _ => return Err(DebugError::Unsupported.into()),
        };
        output.push(self.response(request.seq, &request.command, true, None, body)?);
        Ok(())
    }

    pub(super) fn poll_target_sessions(
        &mut self,
        output: &mut Vec<Vec<u8>>,
    ) -> Result<(), DapError> {
        let Some(mut targets) = self.targets.take() else {
            return Ok(());
        };
        let result = (|| {
            let mut index = 0;
            while index < targets.children.len() {
                let (id, child) = &mut targets.children[index];
                let id = *id;
                let result = child
                    .poll()
                    .and_then(|frames| self.wrap_target_frames(id, frames));
                match result {
                    Ok(frames) => {
                        output.extend(frames);
                        if targets.children[index].1.is_disconnected() {
                            targets.children.remove(index);
                        } else {
                            index += 1;
                        }
                    }
                    Err(error) => {
                        let (_, mut child) = targets.children.remove(index);
                        let _ = child.end_input();
                        output.push(self.target_closed(id, &error)?);
                    }
                }
            }
            Ok(())
        })();
        self.targets = Some(targets);
        result
    }

    fn wrap_target_frames(
        &mut self,
        id: DebugTargetId,
        frames: Vec<Vec<u8>>,
    ) -> Result<Vec<Vec<u8>>, DapError> {
        let mut output = Vec::new();
        output
            .try_reserve_exact(frames.len())
            .map_err(|_| DapError::Capacity)?;
        for frame in frames {
            let offset = frame
                .windows(4)
                .position(|part| part == b"\r\n\r\n")
                .ok_or_else(|| protocol("invalid child DAP frame"))?
                + 4;
            // Keep the already-encoded JSON intact, including escaped lone
            // surrogates that serde_json::Value cannot represent losslessly.
            let message: Box<RawValue> = serde_json::from_slice(&frame[offset..])
                .map_err(|_| protocol("invalid child DAP message"))?;
            output.push(self.event(
                "jetbunny/targetMessage",
                obj(vec![
                    ("targetId", text(&id.wire_id())),
                    ("message", Value::Raw(message)),
                ]),
            )?);
        }
        Ok(output)
    }

    fn target_closed(&mut self, id: DebugTargetId, error: &DapError) -> Result<Vec<u8>, DapError> {
        self.event(
            "jetbunny/targetSessionClosed",
            obj(vec![
                ("targetId", text(&id.wire_id())),
                ("message", text(&error.to_string())),
            ]),
        )
    }
}

impl Drop for DapAdapter {
    fn drop(&mut self) {
        self.detach_target_sessions();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_nested_messages_preserve_surrogates_and_share_the_output_budget() {
        let raw: Box<RawValue> = serde_json::from_str(r#"{"result":"\ud800","seq":1}"#).unwrap();
        let value = obj(vec![("message", Value::Raw(raw))]);
        let framed = frame_value(&value, 128).unwrap();
        let offset = framed.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        assert_eq!(
            &framed[offset..],
            br#"{"message":{"result":"\ud800","seq":1}}"#
        );
        assert!(matches!(frame_value(&value, 8), Err(DapError::Capacity)));
    }
}

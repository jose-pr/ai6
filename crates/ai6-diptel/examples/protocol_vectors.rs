use ai6_diptel::{
    LifecycleEvent, MissionId, OperationId, SessionId, SessionState, TerminalData,
    TerminalDirection, TerminalResize, lifecycle_frames, resize_frames, terminal_frames,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let lifecycle = LifecycleEvent {
        operation_id: OperationId::new("station-a")?,
        mission_id: MissionId::new("mission-17")?,
        session_id: SessionId::new("provider-run-3")?,
        state: SessionState::Working,
        sequence: 4,
    };
    let terminal = TerminalData::new(
        SessionId::new("provider-run-3")?,
        5,
        TerminalDirection::Output,
        vec![0xff, 0x00, 0x80],
    )?;
    let resize = TerminalResize::new(SessionId::new("provider-run-3")?, 6, 120, 40)?;

    for (name, frame) in [
        ("lifecycle_schema", lifecycle_frames(&lifecycle)?.0),
        ("lifecycle_value", lifecycle_frames(&lifecycle)?.1),
        ("terminal_schema", terminal_frames(&terminal)?.0),
        ("terminal_value", terminal_frames(&terminal)?.1),
        ("resize_schema", resize_frames(&resize)?.0),
        ("resize_value", resize_frames(&resize)?.1),
    ] {
        println!("{name}={}", hex(&frame));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

//! Hook-derived presentation hints, never proof of model or tool execution.

use aw_contracts::events::EventName;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub(super) enum Activity {
    #[default]
    Unknown,
    Idle,
    Working,
    Blocked,
}

impl Activity {
    pub(super) fn observe(&mut self, event: EventName, stop_handler: bool) {
        use EventName::*;
        *self = match event {
            SessionStart => Self::Idle,
            InputSubmit | ToolBefore | ToolAfter => Self::Working,
            PermissionRequest => Self::Blocked,
            TurnStop if stop_handler => Self::Unknown,
            TurnStop => Self::Idle,
            SessionEnd => Self::Unknown,
            // Subagent completion and compaction cannot end the main turn.
            _ => *self,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_tracks_main_callbacks_without_claiming_continuation_or_child_completion() {
        let mut activity = Activity::default();
        for (event, expected) in [
            (EventName::SessionStart, Activity::Idle),
            (EventName::InputSubmit, Activity::Working),
            (EventName::PermissionRequest, Activity::Blocked),
            (EventName::ToolAfter, Activity::Working),
            (EventName::SubagentStop, Activity::Working),
            (EventName::TurnStop, Activity::Idle),
            (EventName::InputSubmit, Activity::Working),
            (EventName::SessionEnd, Activity::Unknown),
        ] {
            activity.observe(event, false);
            assert_eq!(activity, expected);
        }
        activity.observe(EventName::TurnStop, true);
        assert_eq!(activity, Activity::Unknown);
        assert_eq!(
            serde_json::from_str::<super::super::State>(
                r#"{"session_id":null,"attachment":0,"attached":false,"retired_sessions":[],"occurrences":0,"gap":false}"#
            ).unwrap().activity,
            Activity::Unknown
        );
    }
}

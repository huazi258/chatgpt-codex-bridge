#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    WaitingForChatGptPlan,
    StartingCodexTurn,
    CodexRunning,
    WaitingForChatGptReview,
    PausedForAcceptance,
    Blocked,
    Completed,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    SendPlanningRequest,
    StartCodexTurn { round: u32, codex_prompt: String },
    SendReviewRequest { round: u32, codex_summary: String },
    PauseForAcceptance { reason: String },
    Block { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    pub phase: Phase,
    pub completed_rounds: u32,
    pub pause_after_current_turn: bool,
}

impl Runtime {
    pub fn start() -> (Self, Action) {
        (
            Self {
                phase: Phase::WaitingForChatGptPlan,
                completed_rounds: 0,
                pause_after_current_turn: false,
            },
            Action::SendPlanningRequest,
        )
    }

    pub fn receive_next_task(&mut self, codex_prompt: String) -> Result<Action, String> {
        if !matches!(
            self.phase,
            Phase::WaitingForChatGptPlan | Phase::WaitingForChatGptReview
        ) {
            return Err(
                "NEXT_TASK is only valid while waiting for ChatGPT planning or review".into(),
            );
        }
        if codex_prompt.trim().is_empty() {
            return Err("NEXT_TASK requires a non-empty Codex prompt".into());
        }
        self.phase = Phase::StartingCodexTurn;
        Ok(Action::StartCodexTurn {
            round: self.completed_rounds + 1,
            codex_prompt,
        })
    }

    pub fn codex_started(&mut self) -> Result<(), String> {
        if self.phase != Phase::StartingCodexTurn {
            return Err("Codex can start only after a valid NEXT_TASK".into());
        }
        self.phase = Phase::CodexRunning;
        Ok(())
    }

    pub fn codex_completed(&mut self, codex_summary: String) -> Result<Action, String> {
        if self.phase != Phase::CodexRunning {
            return Err("Codex completion is only valid for a running turn".into());
        }
        self.completed_rounds += 1;
        self.phase = Phase::WaitingForChatGptReview;
        Ok(Action::SendReviewRequest {
            round: self.completed_rounds,
            codex_summary,
        })
    }

    pub fn request_pause_after_current_turn(&mut self) -> Result<(), String> {
        if self.phase != Phase::CodexRunning {
            return Err("a deferred budget pause requires a running Codex turn".into());
        }
        self.pause_after_current_turn = true;
        Ok(())
    }

    pub fn pause(&mut self, reason: String) -> Action {
        self.phase = Phase::PausedForAcceptance;
        Action::PauseForAcceptance { reason }
    }

    pub fn receive_module_done(&mut self, reason: String) -> Result<Action, String> {
        if !matches!(
            self.phase,
            Phase::WaitingForChatGptPlan | Phase::WaitingForChatGptReview
        ) {
            return Err(
                "MODULE_DONE is only valid while waiting for ChatGPT planning or review".into(),
            );
        }
        Ok(self.pause(reason))
    }

    pub fn receive_pause(&mut self, reason: String) -> Action {
        self.pause(reason)
    }

    pub fn block(&mut self, reason: String) -> Action {
        self.phase = Phase::Blocked;
        Action::Block { reason }
    }

    pub fn approve(&mut self) -> Result<(), String> {
        if self.phase != Phase::PausedForAcceptance {
            return Err("approval is only valid at an acceptance pause".into());
        }
        self.phase = Phase::Completed;
        Ok(())
    }

    pub fn continue_after_pause(&mut self) -> Result<(), String> {
        if !matches!(self.phase, Phase::PausedForAcceptance | Phase::Blocked) {
            return Err("continue is only valid after a pause or block".into());
        }
        self.phase = Phase::WaitingForChatGptPlan;
        self.pause_after_current_turn = false;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), String> {
        if matches!(self.phase, Phase::CodexRunning | Phase::StartingCodexTurn) {
            return Err(
                "a running Codex turn cannot be interrupted; it will pause after completion".into(),
            );
        }
        self.phase = Phase::Stopped;
        Ok(())
    }

    #[cfg(test)]
    pub fn recover_after_restart(&mut self) -> Option<Action> {
        if matches!(
            self.phase,
            Phase::StartingCodexTurn
                | Phase::CodexRunning
                | Phase::WaitingForChatGptPlan
                | Phase::WaitingForChatGptReview
        ) {
            Some(self.pause("应用重启时存在未完成自动化；已安全暂停等待验收。".into()))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulated_two_turn_loop_reaches_module_acceptance_pause() {
        let (mut runtime, action) = Runtime::start();
        assert_eq!(action, Action::SendPlanningRequest);

        let action = runtime
            .receive_next_task("Implement task one.".into())
            .expect("first task");
        assert!(matches!(action, Action::StartCodexTurn { round: 1, .. }));
        runtime.codex_started().expect("start first turn");
        let action = runtime
            .codex_completed("first summary".into())
            .expect("first completion");
        assert!(matches!(action, Action::SendReviewRequest { round: 1, .. }));

        let action = runtime
            .receive_next_task("Implement task two.".into())
            .expect("second task");
        assert!(matches!(action, Action::StartCodexTurn { round: 2, .. }));
        runtime.codex_started().expect("start second turn");
        runtime
            .codex_completed("second summary".into())
            .expect("second completion");

        let action = runtime
            .receive_module_done("ChatGPT reported MODULE_DONE.".into())
            .expect("module done is valid after review");
        assert!(matches!(action, Action::PauseForAcceptance { .. }));
        assert_eq!(runtime.phase, Phase::PausedForAcceptance);
        assert_eq!(runtime.completed_rounds, 2);
    }

    #[test]
    fn approval_continue_and_stop_require_safe_states() {
        let (mut runtime, _) = Runtime::start();
        assert!(runtime.approve().is_err());
        runtime.receive_pause("needs confirmation".into());
        runtime.approve().expect("paused module can be approved");
        assert_eq!(runtime.phase, Phase::Completed);

        let (mut runtime, _) = Runtime::start();
        runtime.receive_pause("needs confirmation".into());
        runtime
            .continue_after_pause()
            .expect("paused module can continue");
        assert_eq!(runtime.phase, Phase::WaitingForChatGptPlan);
        runtime.stop().expect("waiting module can stop");
        assert_eq!(runtime.phase, Phase::Stopped);
    }

    #[test]
    fn restart_pauses_an_in_progress_orchestration() {
        let (mut runtime, _) = Runtime::start();
        let action = runtime.recover_after_restart().expect("must pause");
        assert!(matches!(action, Action::PauseForAcceptance { .. }));
        assert_eq!(runtime.phase, Phase::PausedForAcceptance);
    }
}

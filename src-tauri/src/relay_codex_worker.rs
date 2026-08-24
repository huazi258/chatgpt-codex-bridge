use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

pub enum RelayCodexTransportEvent {
    Message(Value),
    Timeout,
    Closed(String),
    ProtocolError(String),
}

pub struct RelayCodexProcessTransport {
    child: Child,
    stdin: std::process::ChildStdin,
    events: mpsc::Receiver<Result<Value, String>>,
}

impl RelayCodexProcessTransport {
    pub fn spawn(command: &str, working_directory: &str) -> Result<Self, String> {
        let mut child = Command::new(command)
            .arg("app-server")
            .current_dir(working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("无法启动本地 Codex App Server：{error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Codex App Server 没有可用输入流。".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Codex App Server 没有可用输出流。".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "Codex App Server 没有可用错误流。".to_string())?;
        let (sender, events) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let value = line
                    .map_err(|error| format!("无法读取 Codex 输出：{error}"))
                    .and_then(|line| {
                        serde_json::from_str(&line)
                            .map_err(|error| format!("Codex 输出不是 JSON：{error}"))
                    });
                if sender.send(value).is_err() {
                    break;
                }
            }
        });
        std::thread::spawn(move || for _ in BufReader::new(stderr).lines() {});
        Ok(Self {
            child,
            stdin,
            events,
        })
    }

    pub fn send_json(&mut self, value: Value) -> Result<(), String> {
        serde_json::to_writer(&mut self.stdin, &value)
            .map_err(|error| format!("无法编码 Codex 请求：{error}"))?;
        self.stdin
            .write_all(b"\n")
            .map_err(|error| format!("无法发送 Codex 请求：{error}"))?;
        self.stdin
            .flush()
            .map_err(|error| format!("无法刷新 Codex 请求：{error}"))
    }

    pub fn recv_event(&self, timeout: Duration) -> RelayCodexTransportEvent {
        match self.events.recv_timeout(timeout) {
            Ok(Ok(value)) => RelayCodexTransportEvent::Message(value),
            Ok(Err(error)) => RelayCodexTransportEvent::ProtocolError(error),
            Err(mpsc::RecvTimeoutError::Timeout) => RelayCodexTransportEvent::Timeout,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                RelayCodexTransportEvent::Closed("Codex App Server 已在回合完成前退出。".into())
            }
        }
    }

    pub fn shutdown(mut self) -> Result<(), String> {
        drop(self.stdin);
        let _ = self.child.kill();
        self.child
            .wait()
            .map(|_| ())
            .map_err(|error| format!("无法结束 Codex 对话：{error}"))
    }
}

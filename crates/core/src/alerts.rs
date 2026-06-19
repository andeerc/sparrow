use serde_json;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{error, info, warn};

use lettre::message::Message;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

use crate::state::StateStore;

pub struct AlertEvent {
    pub id: String,
    pub metric: String,
    pub value: f64,
    pub threshold: f64,
    pub operator: String,
    pub severity: String,
    pub message: String,
    pub timestamp: i64,
}

pub struct AlertChannel {
    pub id: String,
    pub channel_type: String,
    pub config: serde_json::Value,
    pub enabled: bool,
}

pub struct AlertDispatch {
    rx: tokio::sync::mpsc::Receiver<AlertEvent>,
    channels: Vec<AlertChannel>,
    state: Arc<StateStore>,
}

impl AlertDispatch {
    pub fn new(
        rx: tokio::sync::mpsc::Receiver<AlertEvent>,
        channels: Vec<AlertChannel>,
        state: Arc<StateStore>,
    ) -> Self {
        Self {
            rx,
            channels,
            state,
        }
    }

    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            self.run().await;
        })
    }

    async fn run(mut self) {
        while let Some(event) = self.rx.recv().await {
            info!(
                event_id = %event.id,
                metric = %event.metric,
                severity = %event.severity,
                "received alert event"
            );

            let matching: Vec<&AlertChannel> = self
                .channels
                .iter()
                .filter(|c| c.enabled)
                .filter(|c| c.channel_type == "telegram" || c.channel_type == "email")
                .collect();

            if matching.is_empty() {
                warn!(
                    event_id = %event.id,
                    "no enabled channels configured for alert"
                );
                continue;
            }

            for channel in &matching {
                let event = event.clone_for_dispatch();
                let channel_id = channel.id.clone();
                let channel_type = channel.channel_type.clone();
                let config = channel.config.clone();

                let result = dispatch_with_retry(channel_type.clone(), &config, &event).await;
                match result {
                    Ok(()) => {
                        info!(
                            channel_id = %channel_id,
                            event_id = %event.id,
                            "alert dispatched successfully"
                        );
                        let _ = self.state.record_alert_event(
                            &channel_id,
                            &channel_type,
                            &event.metric,
                            event.value,
                            event.threshold,
                            &event.message,
                            &event.severity,
                        );
                    }
                    Err(e) => error!(
                        channel_id = %channel_id,
                        event_id = %event.id,
                        error = %e,
                        "failed to dispatch alert after retries"
                    ),
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct DispatchEvent {
    pub id: String,
    pub metric: String,
    pub value: f64,
    pub threshold: f64,
    pub operator: String,
    pub severity: String,
    pub message: String,
    pub timestamp: i64,
}

impl AlertEvent {
    fn clone_for_dispatch(&self) -> DispatchEvent {
        DispatchEvent {
            id: self.id.clone(),
            metric: self.metric.clone(),
            value: self.value,
            threshold: self.threshold,
            operator: self.operator.clone(),
            severity: self.severity.clone(),
            message: self.message.clone(),
            timestamp: self.timestamp,
        }
    }
}

pub struct TelegramSender {
    pub bot_token: String,
    pub chat_id: String,
    pub http_client: reqwest::Client,
}

impl TelegramSender {
    pub fn new(bot_token: String, chat_id: String) -> Self {
        Self {
            bot_token,
            chat_id,
            http_client: reqwest::Client::new(),
        }
    }

    pub async fn telegram_send(&self, event: &DispatchEvent) -> anyhow::Result<bool> {
        let text = format!(
            "⚠️ [{}] {}: {:.2} (threshold: {:.2})",
            event.severity, event.metric, event.value, event.threshold
        );

        let url = format!("https://api.telegram.org/bot{}/sendMessage", self.bot_token);

        let body = serde_json::json!({
            "chat_id": self.chat_id,
            "text": text,
        });

        let mut attempt = 0u32;
        let max_retries = 3;

        loop {
            attempt += 1;
            let resp = self.http_client.post(&url).json(&body).send().await?;

            if resp.status().is_success() {
                return Ok(true);
            }

            if resp.status().as_u16() == 429 {
                if attempt >= max_retries {
                    anyhow::bail!("telegram rate limited after {} retries", max_retries);
                }
                warn!("telegram rate limited (429), retrying in 5s");
                sleep(Duration::from_secs(5)).await;
                continue;
            }

            let status = resp.status();
            let body_text = resp.text().await?;
            anyhow::bail!("telegram API error: {} - {}", status, body_text);
        }
    }
}

pub struct EmailSender {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub username: String,
    pub password: String,
    pub from: String,
    pub to: String,
}

impl EmailSender {
    pub fn new(
        smtp_host: String,
        smtp_port: u16,
        username: String,
        password: String,
        from: String,
        to: String,
    ) -> Self {
        Self {
            smtp_host,
            smtp_port,
            username,
            password,
            from,
            to,
        }
    }

    pub async fn email_send(&self, event: &DispatchEvent) -> anyhow::Result<bool> {
        let subject = format!(
            "[Sparrow Alert] [{}] {}: {:.2}",
            event.severity, event.metric, event.value
        );

        let body = format!(
            "Sparrow Alert\n\
             \n\
             Severity: {}\n\
             Metric: {}\n\
             Value: {:.2}\n\
             Threshold: {:.2}\n\
             Operator: {}\n\
             Timestamp: {}\n\
             \n\
             Message: {}",
            event.severity,
            event.metric,
            event.value,
            event.threshold,
            event.operator,
            event.timestamp,
            event.message,
        );

        let email = Message::builder()
            .from(self.from.parse()?)
            .to(self.to.parse()?)
            .subject(&subject)
            .body(body)?;

        let creds = Credentials::new(self.username.clone(), self.password.clone());

        let mailer: AsyncSmtpTransport<Tokio1Executor> = if self.smtp_port == 465 {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&self.smtp_host)?
                .port(self.smtp_port)
                .credentials(creds)
                .build()
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.smtp_host)?
                .port(self.smtp_port)
                .credentials(creds)
                .build()
        };

        let max_retries = 3u32;
        let mut attempt = 0u32;

        loop {
            attempt += 1;
            match mailer.send(email.clone()).await {
                Ok(_) => return Ok(true),
                Err(e) => {
                    if attempt >= max_retries {
                        anyhow::bail!("email send failed after {} retries: {}", max_retries, e);
                    }
                    warn!(
                        "email send failed (attempt {}/{}), retrying in 5s: {}",
                        attempt, max_retries, e
                    );
                    sleep(Duration::from_secs(5)).await;
                }
            }
        }
    }
}

const MAX_RETRIES: u32 = 3;
const BACKOFF_SECS: u64 = 5;

async fn dispatch_with_retry(
    channel_type: String,
    config: &serde_json::Value,
    event: &DispatchEvent,
) -> anyhow::Result<()> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match try_dispatch(&channel_type, config, event).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                if attempt >= MAX_RETRIES {
                    return Err(e);
                }
                warn!(
                    channel_type = %channel_type,
                    attempt,
                    max_retries = MAX_RETRIES,
                    error = %e,
                    "dispatch failed, retrying"
                );
                sleep(Duration::from_secs(BACKOFF_SECS)).await;
            }
        }
    }
}

async fn try_dispatch(
    channel_type: &str,
    config: &serde_json::Value,
    event: &DispatchEvent,
) -> anyhow::Result<()> {
    match channel_type {
        "telegram" => {
            let bot_token = config["bot_token"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("missing bot_token in telegram config"))?;
            let chat_id = config["chat_id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("missing chat_id in telegram config"))?;
            let sender = TelegramSender::new(bot_token.to_string(), chat_id.to_string());
            sender.telegram_send(event).await?;
            Ok(())
        }
        "email" => {
            info!(
                event_id = %event.id,
                channel = "email",
                "dispatching via email"
            );
            let smtp_host = config
                .get("smtp_host")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("missing smtp_host in email channel config"))?
                .to_string();
            let smtp_port = config
                .get("smtp_port")
                .and_then(|v| v.as_u64())
                .unwrap_or(587) as u16;
            let username = config
                .get("username")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("missing username in email channel config"))?
                .to_string();
            let password = config
                .get("password")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("missing password in email channel config"))?
                .to_string();
            let from = config
                .get("from")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("missing from in email channel config"))?
                .to_string();
            let to = config
                .get("to")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("missing to in email channel config"))?
                .to_string();
            let sender = EmailSender::new(smtp_host, smtp_port, username, password, from, to);
            sender.email_send(event).await?;
            Ok(())
        }
        other => {
            anyhow::bail!("unsupported channel type: {}", other);
        }
    }
}

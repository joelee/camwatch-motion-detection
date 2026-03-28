//! MQTT publishing runtime.
//!
//! `rumqttc` uses one side for publishing and another side for driving the connection state. The
//! dedicated threads here keep those responsibilities separate from the frame-processing thread.

use std::{sync::mpsc, thread, time::Duration};

use rumqttc::{Client, Connection, MqttOptions, QoS};
use thiserror::Error;
use tracing::{debug, error, warn};

use crate::config::MotionDetectionConfig;

pub struct MqttRuntime {
    sender: mpsc::Sender<Vec<u8>>,
    publish_handle: thread::JoinHandle<()>,
    event_handle: thread::JoinHandle<()>,
}

impl MqttRuntime {
    pub fn sender(&self) -> mpsc::Sender<Vec<u8>> {
        self.sender.clone()
    }

    pub fn shutdown(self) -> Result<(), MqttRuntimeError> {
        drop(self.sender);

        self.publish_handle
            .join()
            .map_err(|_| MqttRuntimeError::PublishThread)?;
        self.event_handle
            .join()
            .map_err(|_| MqttRuntimeError::EventThread)?;

        Ok(())
    }
}

pub fn start(settings: &MotionDetectionConfig) -> Result<MqttRuntime, MqttError> {
    let mut options = MqttOptions::new(
        settings.mqtt_client_id.clone(),
        settings.mqtt_host.clone(),
        settings.mqtt_port,
    );
    options.set_keep_alive(Duration::from_secs(settings.mqtt_keep_alive_seconds));

    if let Some(username) = settings
        .mqtt_username
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        let password = settings
            .mqtt_password
            .as_deref()
            .unwrap_or_default()
            .to_owned();
        options.set_credentials(username, password);
    }

    let qos = qos_from_u8(settings.mqtt_qos)?;
    let topic = settings.mqtt_topic.clone();
    // `Client` is used by our publish thread; `Connection` must keep polling in the background or
    // the MQTT session will stall.
    let (client, connection) = Client::new(options, 10);
    let (sender, receiver) = mpsc::channel::<Vec<u8>>();

    let publish_handle = spawn_publish_thread(client, receiver, topic, qos);
    let event_handle = spawn_event_thread(connection);

    Ok(MqttRuntime {
        sender,
        publish_handle,
        event_handle,
    })
}

fn spawn_publish_thread(
    client: Client,
    receiver: mpsc::Receiver<Vec<u8>>,
    topic: String,
    qos: QoS,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("mqtt-publisher".to_owned())
        .spawn(move || {
            // The standard channel gives the rest of the app a simple fire-and-forget way to queue
            // outbound MQTT payloads.
            for payload in receiver {
                if let Err(error) = client.publish(topic.clone(), qos, false, payload) {
                    error!(?error, "failed to publish motion event to mqtt");
                }
            }

            if let Err(error) = client.disconnect() {
                warn!(?error, "failed to disconnect mqtt client cleanly");
            }
        })
        .unwrap_or_else(|error| panic!("failed to spawn mqtt publish thread: {error}"))
}

fn spawn_event_thread(mut connection: Connection) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("mqtt-events".to_owned())
        .spawn(move || {
            // `connection.iter()` drives the underlying socket and keep-alive processing.
            for notification in connection.iter() {
                match notification {
                    Ok(notification) => debug!(?notification, "mqtt event"),
                    Err(error) => {
                        warn!(?error, "mqtt event loop ended");
                        break;
                    }
                }
            }
        })
        .unwrap_or_else(|error| panic!("failed to spawn mqtt event thread: {error}"))
}

fn qos_from_u8(value: u8) -> Result<QoS, MqttError> {
    match value {
        0 => Ok(QoS::AtMostOnce),
        1 => Ok(QoS::AtLeastOnce),
        2 => Ok(QoS::ExactlyOnce),
        _ => Err(MqttError::InvalidQos(value)),
    }
}

#[derive(Debug, Error)]
pub enum MqttError {
    #[error("invalid mqtt qos level: {0}")]
    InvalidQos(u8),
}

#[derive(Debug, Error)]
pub enum MqttRuntimeError {
    #[error("mqtt publish thread failed")]
    PublishThread,
    #[error("mqtt event thread failed")]
    EventThread,
}

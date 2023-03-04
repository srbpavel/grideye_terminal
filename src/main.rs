use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;

use std::ops;

use std::time::Instant;
use std::time::Duration;

use chrono::DateTime;
use chrono::Utc;
use chrono::Local;

use uuid::Uuid;

use rumqttc::Client;
use rumqttc::MqttOptions;
use rumqttc::Packet;
use rumqttc::QoS;

#[allow(unused)]
use log::info;
#[allow(unused)]
use log::warn;
#[allow(unused)]
use log::error;

//CONFIG
#[derive(Debug)]
#[toml_cfg::toml_config]
pub struct Config {
    //#[default("localhost")]
    //#[default("spongebob")]
    #[default("192.168.0.103")]
    mqtt_host: &'static str,
    #[default("")]
    mqtt_user: &'static str,
    #[default("")]
    mqtt_pass: &'static str,
}

const FLAG_DEBUG: bool = false;

const CHUNK_SIZE: usize = 4;

const TEMPERATURE_ERROR_VALUE: f32 = 86.0;
const TEMPERATURE_DEFAULT_VALUE: f32 = 126.0;
const TEMPERATURE_MAX: f32 = -55.0;
const TEMPERATURE_MIN: f32 = 125.0;
const TEMPERATURE_BOUNDARY_NEGATIVE: f32 = 10.0;

const LEN: usize = 8;
const POW: usize = LEN * LEN;
const MQTT_PAYLOAD_SIZE: usize = POW * CHUNK_SIZE;

const MQTT_PORT: u16 = 1883;
const MQTT_TOPIC: &str = "/grid_eye/";
const MQTT_CLIENT_ID: &str = "grideye_terminal";

pub type Temperature = f32;

struct Boundary {
    value: Temperature,
    datetime: DateTime<Local>,
}

impl Display for Boundary {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(
            f,
            "{:0.02} {}",
             self.value,
             self.datetime,
        )
    }
}

type HeatArray<T, const N: usize> = [[T; N]; N];

#[derive(Debug)]
pub struct HeatMap<T, const N: usize>(pub HeatArray<T, N>);

impl<T, const N: usize> ops::Deref for HeatMap<T, N> {
    type Target = HeatArray<T, N>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T, const N: usize> Display for HeatMap<T, N>
where
    T: Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut line = 0;
        let cell = N * 6;
        let blank_line = format!("\n*{}*", " ".repeat((cell) + 3));

        self.iter().fold(Ok(()), |result, row| {
            result.and_then(|_| {
                line += 1;

                let first = if line.eq(&1) {
                    format!("{}*{blank_line}\n", "* ".repeat(((cell) + 5) / 2))
                } else {
                    String::from("")
                };

                let last = if line.eq(&self.len()) {
                    format!("\n{}*", "* ".repeat(((cell) + 5) / 2))
                } else {
                    String::from("")
                };

                writeln!(
                    f,
                    "{first}* {}  *{blank_line}{last}",
                    row.iter()
                        .map(|t| { format!(" {t:0.02}") })
                        .collect::<String>(),
                )
            })
        })
    }
}

//
// N is array len, N = LEN * LEN
// L is LEN aka ROW/COLUMN size
impl<const N: usize, const L: usize> TryFrom<[Temperature; N]> for HeatMap<Temperature, L> {
    type Error = &'static str;
    
    fn try_from(array: [Temperature; N]) -> Result<Self, Self::Error> {
        let mut heat_map = HeatMap::<Temperature, L>([[TEMPERATURE_DEFAULT_VALUE; L]; L]);
        
        let mut index = 0;

        let sqrt = (N as f32).sqrt();
        let len = if sqrt.eq(&sqrt.floor()) {
            sqrt as usize
        } else {
            return Err("error try_from 1d array to square")
        };

        (0..len as u8).into_iter().for_each(|x| {
            (0..len as u8).into_iter().for_each(|y| {
                if let Some(pixel) = array.get(index) {
                    if let Some(x_row) = heat_map.0.get(x as usize) {
                        if x_row.get(y as usize).is_some() {
                            heat_map.0[x as usize][y as usize] = *pixel;
                        }
                    }
                }
                
                index += 1;
            })
        });

        Ok(heat_map)
    }
}

//
fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    warn!("GRIDEYE_TERMINAL");
    
    let mut cycle_counter = 0;
    let mut invalid_data = vec![];
    let mut negative_data = vec![];
    let mut max_temperature_total = Boundary { value: TEMPERATURE_MAX, datetime: Local::now()};
    let mut min_temperature_total = Boundary { value: TEMPERATURE_MIN, datetime: Local::now()};

    let mqtt_uniq_id = format!("{}_{}",
            MQTT_CLIENT_ID,
            Uuid::new_v4().simple(),
    );
    
    let mut mqttoptions = MqttOptions::new(
        mqtt_uniq_id,
        CONFIG.mqtt_host,
        MQTT_PORT,
    );

    mqttoptions.set_credentials(CONFIG.mqtt_user, CONFIG.mqtt_pass);
    mqttoptions.set_keep_alive(Duration::from_secs(5));
    let (mut mqtt_client, mut mqtt_connection) = Client::new(mqttoptions, 10);
    mqtt_client.subscribe(MQTT_TOPIC, QoS::AtMostOnce)?;

    for (_, notification) in mqtt_connection.iter().enumerate() {
        if FLAG_DEBUG {
            info!("Notification = {:#?}", notification);
        };
      
        if let Ok(rumqttc::Event::Incoming(Packet::Publish(publish_data))) = notification {
            if FLAG_DEBUG {
                info!("mqtt_msg: {:?}", publish_data);
            }

            cycle_counter += 1;
            
            let now_utc: DateTime<Utc> = Utc::now();
            let now_local: DateTime<Local> = Local::now();
            warn!("");
            info!("[{cycle_counter}] {now_utc} / {now_local}");
           
            if publish_data.topic.eq(&MQTT_TOPIC) && publish_data.payload.len().eq(&MQTT_PAYLOAD_SIZE) {
                if FLAG_DEBUG {
                    info!("payload [{}]: {:?}\n",
                          publish_data.payload.len(),
                          &publish_data.payload as &[u8],
                    );
                }

                let mut max_temperature = TEMPERATURE_MAX;
                let mut min_temperature = TEMPERATURE_MIN;
                
                let start = Instant::now();
                let chunks = publish_data.payload.chunks(CHUNK_SIZE);
                let array: [f32; LEN * LEN] = chunks
                    .enumerate()
                    .map(|(index, chunk)| {
                        let chunk_result: Result<[u8; 4], _> = chunk.try_into();

                        match chunk_result {
                            Ok(value) => {
                                let temp = f32::from_be_bytes(value);

                                // ACTUAL
                                if temp > max_temperature {
                                    max_temperature = temp;
                                }

                                if temp < min_temperature {
                                    min_temperature = temp;
                                }

                                // TOTAL
                                if temp > max_temperature_total.value {
                                    max_temperature_total.value = temp;
                                    max_temperature_total.datetime = Local::now();
                                    
                                }

                                if temp < min_temperature_total.value {
                                    min_temperature_total.value = temp;
                                    min_temperature_total.datetime = Local::now();
                                }

                                // NEGATIVE
                                if temp < TEMPERATURE_BOUNDARY_NEGATIVE {
                                    negative_data.push((index, chunk.to_vec()));
                                }
                                
                                temp
                            },
                            Err(e) => {
                                invalid_data.push((index, chunk.to_vec()));

                                error!("### chunk error conversion: {e:?} {chunk:?}");

                                TEMPERATURE_ERROR_VALUE
                            },
                        }
                    })
                    .collect::<Vec<f32>>()
                     // trait `From<Vec<f32>>` is not implemented for `[f32; 64]`
                    // .into();
                    .try_into()
                    .unwrap();

                let stop = Instant::now();
                warn!("measure durration: {:?}", stop.duration_since(start));
                
                if FLAG_DEBUG {
                    info!("array[{}]: {array:?}\n", array.len());
                }
                    
                let start = Instant::now();
                let heat_map: Result<HeatMap<Temperature, LEN>, &'static str> = HeatMap::try_from(array);
                let stop = Instant::now();
                
                match heat_map {
                    Ok(m) => {
                        info!("min: {min_temperature:0.02} max: {max_temperature:0.02}");

                        info!("total:\nmin: {}max: {}",
                              min_temperature_total,
                              max_temperature_total,
                        );
                       
                        info!("heat_map_display:\n\n{m}");
                    },
                    Err(e) => {
                        error!("array to heat_map failed >>> {e}");
                    },
                }
                warn!("measure durration: {:?}", stop.duration_since(start));

                error!("invalid_chunks: {invalid_data:?}");
                error!("negative_chunks: {negative_data:?}");
            }
        }
    }

    Ok(())
}

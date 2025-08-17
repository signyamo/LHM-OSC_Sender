use std::{fs, net::UdpSocket, path::Path, time::{Duration, Instant}};
use rosc::{OscPacket, OscMessage, OscType};
use chrono::{Local, Datelike};
use reqwest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use eframe::egui::{self, RichText, Pos2, Stroke, Color32};

#[derive(Serialize, Deserialize, Debug)]
struct Config {
    osc_ip: String,
    osc_port: u16,
    json_port: u16,
    cpu_temp_name: String,
    cpu_usage_name: String,
    gpu_temp_name: String,
    gpu_usage_name: String,
    gpu_mem_used_name: String,
    gpu_mem_total_name: String,
    wifi_up_name: String,
    wifi_down_name: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            osc_ip: "127.0.0.1".to_string(),
            osc_port: 9000,
            json_port: 8085,
            cpu_temp_name: "Core (Tctl/Tdie)".to_string(),
            cpu_usage_name: "CPU Total".to_string(),
            gpu_temp_name: "GPU Core_Temp-1  ( ! )".to_string(),
            gpu_usage_name: "GPU Core_Used-1  ( ! )".to_string(),
            gpu_mem_used_name: "GPU Memory_Used-1  ( ! )".to_string(),
            gpu_mem_total_name: "GPU Memory_Total-1  ( ! )".to_string(),
            wifi_up_name: "Upload Speed".to_string(),
            wifi_down_name: "Download Speed".to_string(),
        }
    }
}

impl Config {
    fn load() -> Self {
        let path = Path::new("config.json");
        if path.exists() {
            if let Ok(data) = fs::read_to_string(path) {
                if let Ok(cfg) = serde_json::from_str::<Config>(&data) {
                    return cfg;
                }
            }
        }
        Self::default()
    }
    fn save(&self) {
        let _ = fs::write("config.json", serde_json::to_string_pretty(self).unwrap());
    }
}

fn find_numeric(node: &Value, name: &str) -> Option<f32> {
    if let Some(text) = node.get("Text").and_then(|t| t.as_str()) {
        if text.trim().eq_ignore_ascii_case(name) {
            if let Some(val_str) = node.get("Value").and_then(|v| v.as_str()) {
                let mut parts = val_str.split_whitespace();
                if let (Some(num_str), Some(unit)) = (parts.next(), parts.next()) {
                    if let Ok(mut num) = num_str.parse::<f32>() {
                        match unit.to_lowercase().as_str() {
                            "%" => {}
                            "kb/s" => num /= 1024.0,
                            "mb/s" => {}
                            "gb/s" => num *= 1024.0,
                            _ => {}
                        }
                        return Some(num);
                    }
                } else if let Ok(num) = val_str.parse::<f32>() {
                    return Some(num);
                }
            }
        }
    }
    if let Some(children) = node.get("Children").and_then(|c| c.as_array()) {
        for child in children {
            if let Some(val) = find_numeric(child, name) {
                return Some(val);
            }
        }
    }
    None
}

fn find_temperature(node: &Value, name: &str) -> Option<f32> {
    find_numeric(node, name).filter(|num| *num > -50.0 && *num < 150.0)
}

fn find_wifi_speed(json: &Value, direction: &str) -> Option<f32> {
    if let Some(children) = json.get("Children").and_then(|c| c.as_array()) {
        for child in children {
            if let Some(text) = child.get("Text").and_then(|t| t.as_str()) {
                if text.trim() == "Wi-Fi" {
                    return find_numeric(child, direction);
                }
            }
            if let Some(val) = find_wifi_speed(child, direction) {
                return Some(val);
            }
        }
    }
    None
}

fn get_lhm_json(port: u16) -> Option<Value> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(300))
        .build()
        .unwrap();
    let url = format!("http://localhost:{}/data.json", port);
    match client.get(&url).send() {
        Ok(resp) => resp.json::<Value>().ok(),
        Err(_) => None,
    }
}

fn send_osc_float(sock: &UdpSocket, addr: &str, port: u16, path: &str, value: f32) {
    let packet = OscPacket::Message(OscMessage {
        addr: path.to_string(),
        args: vec![OscType::Float(value)],
    });
    if let Ok(buf) = rosc::encoder::encode(&packet) {
        let _ = sock.send_to(&buf, (addr, port));
    }
}

struct MyApp {
    config: Config,
    input_osc_ip: String,
    input_osc_port: String,
    input_json_port: String,
    cpu_temp: f32,
    cpu_usage: f32,
    gpu_temp: f32,
    gpu_usage: f32,
    gpu_mem_used: f32,
    gpu_mem_total: f32,
    gpu_mem_percent: f32,
    wifi_up: f32,
    wifi_down: f32,
    osc_socket: UdpSocket,
    lhm_running: bool,
    last_fail: Instant,
}

impl Default for MyApp {
    fn default() -> Self {
        let cfg = Config::load();
        Self {
            input_osc_ip: cfg.osc_ip.clone(),
            input_osc_port: cfg.osc_port.to_string(),
            input_json_port: cfg.json_port.to_string(),
            config: cfg,
            cpu_temp: -1.0,
            cpu_usage: -1.0,
            gpu_temp: -1.0,
            gpu_usage: -1.0,
            gpu_mem_used: -1.0,
            gpu_mem_total: -1.0,
            gpu_mem_percent: -1.0,
            wifi_up: 0.0,
            wifi_down: 0.0,
            osc_socket: UdpSocket::bind("0.0.0.0:0").expect("Failed to bind UDP socket"),
            lhm_running: false,
            last_fail: Instant::now() - Duration::from_secs(10),
        }
    }
}

impl eframe::App for MyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let try_fetch = self.last_fail.elapsed() > Duration::from_secs(5);
        if try_fetch {
            if let Some(json) = get_lhm_json(self.config.json_port) {
                self.lhm_running = true;
                self.cpu_temp = find_temperature(&json, &self.config.cpu_temp_name).unwrap_or(-1.0);
                self.cpu_usage = find_numeric(&json, &self.config.cpu_usage_name).unwrap_or(-1.0);
                self.gpu_temp = find_temperature(&json, &self.config.gpu_temp_name).unwrap_or(-1.0);
                self.gpu_usage = find_numeric(&json, &self.config.gpu_usage_name).unwrap_or(-1.0);
                self.gpu_mem_used = find_numeric(&json, &self.config.gpu_mem_used_name).unwrap_or(-1.0);
                self.gpu_mem_total = find_numeric(&json, &self.config.gpu_mem_total_name).unwrap_or(-1.0);
                if self.gpu_mem_used > 0.0 && self.gpu_mem_total > 0.0 {
                    self.gpu_mem_percent = (self.gpu_mem_used / self.gpu_mem_total) * 100.0;
                }
                self.wifi_up = find_wifi_speed(&json, &self.config.wifi_up_name).unwrap_or(0.0);
                self.wifi_down = find_wifi_speed(&json, &self.config.wifi_down_name).unwrap_or(0.0);

                let ip = &self.config.osc_ip;
                let port = self.config.osc_port;


                // LHM-JSONのセンサー値OSC送信パラメーター名 (-> VRChat)
                send_osc_float(&self.osc_socket, ip, port, "/avatar/parameters/CPU_Temp", self.cpu_temp);
                send_osc_float(&self.osc_socket, ip, port, "/avatar/parameters/CPU_Usage", self.cpu_usage);
                send_osc_float(&self.osc_socket, ip, port, "/avatar/parameters/GPU_Temp", self.gpu_temp);
                send_osc_float(&self.osc_socket, ip, port, "/avatar/parameters/GPU_Usage", self.gpu_usage);
                send_osc_float(&self.osc_socket, ip, port, "/avatar/parameters/GPU_Memory_Used", self.gpu_mem_used);
                send_osc_float(&self.osc_socket, ip, port, "/avatar/parameters/GPU_Memory_Percent", self.gpu_mem_percent);
                send_osc_float(&self.osc_socket, ip, port, "/avatar/parameters/Wifi_Up", self.wifi_up);
                send_osc_float(&self.osc_socket, ip, port, "/avatar/parameters/Wifi_Down", self.wifi_down);


                // 時間と曜日を番号に変換して送信 (-> VRChat)
                let now = Local::now();
                let time_numeric = now.format("%H%M%S").to_string().parse::<f32>().unwrap_or(0.0);
                let weekday_numeric = match now.weekday() {
                    chrono::Weekday::Mon => 0.0,
                    chrono::Weekday::Tue => 1.0,
                    chrono::Weekday::Wed => 2.0,
                    chrono::Weekday::Thu => 3.0,
                    chrono::Weekday::Fri => 4.0,
                    chrono::Weekday::Sat => 5.0,
                    chrono::Weekday::Sun => 6.0,
                };

                // 時間と曜日のOSC送信パラメーター名
                send_osc_float(&self.osc_socket, ip, port, "/avatar/parameters/TimeString", time_numeric);
                send_osc_float(&self.osc_socket, ip, port, "/avatar/parameters/Weekday", weekday_numeric);
            } else {
                self.lhm_running = false;
                self.last_fail = Instant::now();
            }
        }


        // ーーーーーーーーーーーーーーーーーーーーーーーーーーー
        // ーーーーー↓↓以下　UI表示関係 ↓↓ーーーーー
        // ーーーーーーーーーーーーーーーーーーーーーーーーーーー


        egui::CentralPanel::default().show(ctx, |ui| {


            // GUI表示（文字列の曜日）
            let now = Local::now();
            let (weekday_str, weekday_numeric) = match now.weekday() {
                chrono::Weekday::Mon => ("Monday", "0.0"),
                chrono::Weekday::Tue => ("Tuesday", "1.0"),
                chrono::Weekday::Wed => ("Wednesday", "2.0"),
                chrono::Weekday::Thu => ("Thursday", "3.0"),
                chrono::Weekday::Fri => ("Friday", "4.0"),
                chrono::Weekday::Sat => ("Saturday", "5.0"),
                chrono::Weekday::Sun => ("Sunday", "6.0"),
            };

            ui.add_space(10.0);

            ui.horizontal(|ui| {
                ui.label(RichText::new(weekday_str).size(24.0));
                ui.label(RichText::new(format!("-> Day of week OSC = ({})", weekday_numeric,)).color(Color32::from_gray(200)));
            });



            // 時間表示
            ui.label(RichText::new(format!("{}", now.format("%H:%M:%S"))).size(36.0));
            ui.add_space(10.0);



            // GUI警告表示の条件分岐（温度）
            fn temp_bg_color(temp: f32) -> Option<Color32> {
                if temp >= 60.0 {
                    Some(Color32::from_rgb(0x9D, 0x27, 0x27))
                } else if temp >= 40.0 {
                    Some(Color32::from_rgb(0x7B, 0x63, 0x00))
                } else {
                    None // 通常時は背景色なし
                }
            }


            // GUI警告表示の条件分岐（使用率）
            fn usage_bg_color(usage: f32) -> Option<Color32> {
                if usage >= 90.0 {
                    Some(Color32::from_rgb(0x9D, 0x27, 0x27)) // 赤
                } else if usage >= 70.0 {
                    Some(Color32::from_rgb(0x7B, 0x63, 0x00)) // 黄
                } else {
                    None
                }
            }



            // ーーー LibreHardwareMonitor - GUI表示処理 ーーー



            // LibreHardwareMonitor起動時の挙動(データ表示)

            if self.lhm_running {
                ui.horizontal(|ui| {
                    ui.label("CPU Temp: ");  // ーーーCPU関連ーーー
                    if let Some(bg) = temp_bg_color(self.cpu_temp) {
                        ui.colored_label(bg, format!("{:.1} °C", self.cpu_temp));
                    } else {
                        ui.label(format!("{:.1} °C", self.cpu_temp));
                    }
                });


                ui.horizontal(|ui| {
                    ui.label("CPU Usage: ");
                    if let Some(bg) = usage_bg_color(self.cpu_usage) {
                        ui.colored_label(bg, format!("{:.1} %", self.cpu_usage));
                    } else {
                        ui.label(format!("{:.1} %", self.cpu_usage));
                    }
                });


                ui.horizontal(|ui| {
                    ui.label("GPU Temp: ");  // ーーーGPU関連ーーー
                    if let Some(bg) = temp_bg_color(self.gpu_temp) {
                        ui.colored_label(bg, format!("{:.1} °C", self.gpu_temp));
                    } else {
                        ui.label(format!("{:.1} °C", self.gpu_temp));
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("GPU Usage: ");
                    if let Some(bg) = usage_bg_color(self.gpu_usage) {
                        ui.colored_label(bg, format!("{:.1} %", self.gpu_usage));
                    } else {
                        ui.label(format!("{:.1} %", self.gpu_usage));
                    }
                });

                // ーーーGPUメモリー関連ーーー
                ui.label(format!("GPU Memory Used: {:.1} MB", self.gpu_mem_used));
                ui.label(format!("GPU Memory Total: {:.1} MB", self.gpu_mem_total));
                ui.horizontal(|ui| {
                    ui.label("GPU Memory Usage: ");
                    if let Some(bg) = usage_bg_color(self.gpu_mem_percent) {
                        ui.colored_label(bg, format!("{:.1} %", self.gpu_mem_percent));
                    } else {
                        ui.label(format!("{:.1} %", self.gpu_mem_percent));
                    }
                });

                // ーーーWi-fi関連ーーー
                ui.label(format!("Wi-Fi Up: {:.2} MB/s", self.wifi_up));
                ui.label(format!("Wi-Fi Down: {:.2} MB/s", self.wifi_down));
            } else {


                // LibreHardwareMonitor未起動時の挙動（エラー表示）
                let elapsed = self.last_fail.elapsed().as_secs_f32();


                 // 再試行の間隔調整 (n秒)
                let retry_time = 5.0;
                let progress = (elapsed / retry_time).clamp(0.0, 1.0);
            

                // ラベル表示と下線描画
                let response = ui.label(
                    RichText::new("LibreHardwareMonitor is not running !!")
                        .color(Color32::RED)
                        .size(20.0),
                );
                
                // 下線を描画
                let rect = response.rect;
                let y = rect.bottom();
                ui.painter().line_segment(
                    [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
                    Stroke::new(2.0, Color32::RED),
                );

                ui.add_space(10.0);

                // 自動更新バーの描画
                ui.add(
                    egui::ProgressBar::new(progress)
                        .fill(egui::Color32::from_gray(240))
                        .text(
                            RichText::new(format!("Auto Retry {:.1} sec", (retry_time - elapsed).max(0.0)))
                                .color(egui::Color32::from_gray(180))
                        ),
                );
            }
        });


        // セッティングのUI関連
        egui::TopBottomPanel::bottom("settings_panel").show(ctx, |ui| {


            // JSON-in と OSC-OUT
            ui.add_space(10.0);
            ui.label("JSON & OSC Settings");

            ui.horizontal(|ui| {
                ui.label("in - JSON Port:");
                ui.text_edit_singleline(&mut self.input_json_port);
            });

            ui.horizontal(|ui| {
                ui.label("out - OSC IP:");
                ui.text_edit_singleline(&mut self.input_osc_ip);
            });
            ui.horizontal(|ui| {
                ui.label("out - OSC Port:");
                ui.text_edit_singleline(&mut self.input_osc_port);
            });


            ui.add_space(10.0);
            ui.separator();
            ui.add_space(10.0);


            // LibreHardwareMonitorのJSON受け取り関係
            ui.label("LHM JSON in Sensor Name");
            ui.horizontal(|ui| {
                ui.label("CPU Temp:");
                ui.text_edit_singleline(&mut self.config.cpu_temp_name);
            });
            ui.horizontal(|ui| {
                ui.label("CPU Usage:");
                ui.text_edit_singleline(&mut self.config.cpu_usage_name);
            });
            ui.horizontal(|ui| {
                ui.label("GPU Temp:");
                ui.text_edit_singleline(&mut self.config.gpu_temp_name);
            });
            ui.horizontal(|ui| {
                ui.label("GPU Usage:");
                ui.text_edit_singleline(&mut self.config.gpu_usage_name);
            });
            ui.horizontal(|ui| {
                ui.label("GPU Memory Used:");
                ui.text_edit_singleline(&mut self.config.gpu_mem_used_name);
            });
            ui.horizontal(|ui| {
                ui.label("GPU Memory Total:");
                ui.text_edit_singleline(&mut self.config.gpu_mem_total_name);
            });
            ui.horizontal(|ui| {
                ui.label("Wi-Fi Upload:");
                ui.text_edit_singleline(&mut self.config.wifi_up_name);
            });
            ui.horizontal(|ui| {
                ui.label("Wi-Fi Download:");
                ui.text_edit_singleline(&mut self.config.wifi_down_name);
            });


            ui.add_space(10.0);


            //設定保存ボタン
            if ui.button("  SAVE  ").clicked() {
                if let (Ok(port), Ok(json_port)) = (
                    self.input_osc_port.parse(),
                    self.input_json_port.parse(),
                ) {
                    self.config.osc_ip = self.input_osc_ip.clone();
                    self.config.osc_port = port;
                    self.config.json_port = json_port;
                    self.config.save();
                }
            }


            ui.add_space(10.0);


        });

        ctx.request_repaint_after(Duration::from_secs(1));
    }
}

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {


        // 初期画面サイズ
        initial_window_size: Some(egui::vec2(380.0, 680.0)),
        ..Default::default()
    };

    // アプリ名
    eframe::run_native(
        "LHM OSC Sender v1",
        options,
        Box::new(|_cc| Box::new(MyApp::default())),
    )
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(windows)]
    {
        let args: Vec<_> = std::env::args().skip(1).collect();
        match args.as_slice() {
            [command,channel,path] if command == "verify-sonar-live-volume" => {
                let channel:jack_sonar::sonar::models::MixerChannel=serde_json::from_value(serde_json::json!(channel))?;
                if !matches!(channel,jack_sonar::sonar::models::MixerChannel::Game|jack_sonar::sonar::models::MixerChannel::Chat|jack_sonar::sonar::models::MixerChannel::Media|jack_sonar::sonar::models::MixerChannel::Aux) {return Err("output group required".into());}
                let mut service=jack_sonar::Service::new()?;let before=service.refresh(false);
                let original=before.volumes.iter().find(|v|v.channel==channel).ok_or("channel missing")?.volume;
                let mut expected=original;let mut samples=vec![];let mut failure=None;
                for step in 1..=6 {
                    let wanted=(original-0.01*step as f64).max(0.0);
                    let started=std::time::Instant::now();
                    match service.set_volume_live(channel,wanted,expected) {
                        Ok(actual)=>{expected=actual;samples.push(serde_json::json!({"requested":wanted,"confirmed":actual,"latency_ms":started.elapsed().as_secs_f64()*1000.0}));}
                        Err(error)=>{failure=Some(error.to_string());break;}
                    }
                    std::thread::sleep(std::time::Duration::from_millis(80));
                }
                let restored=service.set_volume_live(channel,original,expected);
                let after=service.refresh(false);
                let passed=failure.is_none() && restored.is_ok() && before.volumes==after.volumes && before.routes.iter().zip(&after.routes).all(|(a,b)|a.channel==b.channel && a.device_id==b.device_id);
                let report=serde_json::json!({"passed":passed,"scope":"Actual optimized Sonar volume API/readback at the GG 80ms interval; does not prove native pointer/IPC or human listening","channel":channel,"samples":samples,"failure":failure,"restore_error":restored.err().map(|e|e.to_string()),"before_volumes":before.volumes,"after_volumes":after.volumes});
                std::fs::write(path,serde_json::to_vec_pretty(&report)?)?;println!("{}",serde_json::to_string_pretty(&report)?);
                if !passed{return Err("live volume proof failed".into());}
            }
            [command, endpoint, path] if command == "verify-sonar-cache-recovery" => {
                let mut service = jack_sonar::Service::resume_cache_for_diagnostics(endpoint)?;
                let mut samples = vec![];
                let started = std::time::Instant::now();
                while started.elapsed() < std::time::Duration::from_secs(5) {
                    let state = service.refresh(false);
                    samples.push(serde_json::json!({"elapsed_ms":started.elapsed().as_millis(),"connection":state.connection,"message":state.message,"mode":state.mode,"channels":state.sources,"endpoint":service.endpoint_for_diagnostics()}));
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                let disconnected=samples.iter().any(|sample|sample["connection"] != "connected");
                let last=samples.last().ok_or("no samples")?;
                let recovered=last["connection"]=="connected" && last["mode"]=="classic" && last["channels"].as_array().is_some_and(|channels|channels.len()==4) && last["endpoint"].as_str().is_some_and(|current|current!=endpoint);
                let report=serde_json::json!({"passed":disconnected && recovered,"cached_endpoint":endpoint,"read_only":true,"scope":"Previous cached endpoint reconstructed after the actual user GG restart; not a live outage timing measurement","samples":samples});
                std::fs::write(path,serde_json::to_vec_pretty(&report)?)?;
                println!("{}",serde_json::to_string_pretty(&report)?);
                if report["passed"] != true { return Err("cached endpoint recovery was not proven".into()); }
            }
            [command,seconds,path] if command=="watch-sonar" => {
                let seconds:u64=seconds.parse()?;if !(1..=180).contains(&seconds){return Err("watch-sonar seconds must be 1..=180".into());}
                let mut service=jack_sonar::Service::new()?;let started=std::time::Instant::now();let mut samples=vec![];
                while started.elapsed()<std::time::Duration::from_secs(seconds){
                    let state=service.refresh(false);
                    samples.push(serde_json::json!({"elapsed_ms":started.elapsed().as_millis(),"connection":state.connection,"mode":state.mode,"message":state.message,"channels":state.sources,"devices":state.devices,"routes":state.routes,"applications":state.applications,"endpoint":service.endpoint_for_diagnostics()}));
                    std::fs::write(path,serde_json::to_vec_pretty(&samples)?)?;
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
                println!("{}",serde_json::to_string_pretty(&samples)?);
            }
            [command,path] if command=="publisher" => {
                println!("{}",serde_json::to_string_pretty(&serde_json::json!({"path":path,"verified_publisher":jack_windows_audio::identity::verified_publisher(path)}))?);
            }
            [command,pid] if command=="identity" => {
                println!("{}",serde_json::to_string_pretty(&jack_windows_audio::identity::process_identity(pid.parse()?)?)?);
            }
            [command] if command == "source-endpoints" => {
                println!("{}", serde_json::to_string_pretty(&jack_windows_audio::sources::discover()?)?);
            }
            [command] if command == "source-list" => {
                println!("{}",serde_json::to_string_pretty(&jack_windows_audio::virtual_devices::list()?)?);
            }
            [command, inf] if command == "install-sources" => {
                let result=jack_windows_audio::virtual_devices::install_defaults(std::path::Path::new(inf))?;
                println!("{}",serde_json::to_string_pretty(&result)?);
                if !result.all_devices_started {return Err("Jack PnP devices did not start; inspect driver trust/problem codes".into());}
            }
            [command, group, name] if command == "source-create" => {
                let result=jack_windows_audio::virtual_devices::ensure(uuid::Uuid::parse_str(group)?,name)?;
                println!("{}",serde_json::to_string_pretty(&result)?);
                if !result.device.device_started {return Err("Jack source driver is not started".into());}
            }
            [command, group] if command == "source-remove" => {
                jack_windows_audio::virtual_devices::remove(uuid::Uuid::parse_str(group)?)?;
            }
            [command, lab, source, output, seconds] if command == "verify-transport-long" && lab == "--lab" => {
                let report = jack_windows_audio::pcm_proof::verify_transport_long(source,output,seconds.parse()?)?;
                println!("{}",serde_json::to_string_pretty(&report)?);
                if !report.passed { return Err("long native transport proof failed; inspect the report".into()); }
            }
            [command, lab, source, output] if command == "verify-transport" && lab == "--lab" => {
                let report = jack_windows_audio::pcm_proof::verify_transport(source,output)?;
                println!("{}",serde_json::to_string_pretty(&report)?);
                if !report.passed { return Err("native transport proof failed; inspect PCM, gain and lifecycle results".into()); }
            }
            [command, record] if command == "recover-volume" => {
                let report = jack_windows_audio::volume::recover_volume(std::path::Path::new(record))?;
                println!("{}",serde_json::to_string_pretty(&report)?);
                if !report.restored { return Err("recorded volume no longer owned; external change preserved".into()); }
            }
            [command, lab, endpoint] if command == "verify-mixer" && lab == "--lab" => {
                let report = jack_windows_audio::pcm_proof::verify_mixer(endpoint)?;
                println!("{}",serde_json::to_string_pretty(&report)?);
                if !report.passed {return Err("mixer PCM proof failed; inspect before/after gain comparison".into());}
            }
            [command, lab, source, output] if command == "verify-bridge" && lab == "--lab" => {
                let report = jack_windows_audio::pcm_proof::verify_bridge(source,output)?;
                println!("{}",serde_json::to_string_pretty(&report)?);
                if !report.passed {return Err("bridge PCM proof failed; inspect the report".into());}
            }
            [command, lab, endpoint] if command == "verify-pcm" && lab == "--lab" => {
                let report = jack_windows_audio::pcm_proof::verify_pcm(endpoint)?;
                println!("{}",serde_json::to_string_pretty(&report)?);
                if !report.passed {return Err("PCM proof failed; inspect correlation/gain/discontinuities".into());}
            }
            [command, endpoint, backup] if command == "verify-volume" => {
                let report = jack_windows_audio::volume::verify_volume_roundtrip(endpoint,std::path::Path::new(backup))?;
                println!("{}",serde_json::to_string_pretty(&report)?);
                if !report.passed {return Err("volume roundtrip failed; inspect backup/report".into());}
            }
            [command, flow, endpoint, path] if command == "verify-default-policy" || command == "verify-self-app-policy" => {
                let flow = match flow.as_str() {"render" => jack_core::routing::Flow::Render, "capture" => jack_core::routing::Flow::Capture, _ => return Err("flow must be render or capture".into())};
                let report = if command == "verify-default-policy" {jack_windows_audio::policy::verify_default_roundtrip(flow,endpoint,std::path::Path::new(path))?} else {jack_windows_audio::policy::verify_self_app_roundtrip(flow,endpoint,std::path::Path::new(path))?};
                println!("{}", serde_json::to_string_pretty(&report)?);
                if !report.passed { return Err("policy roundtrip failed; inspect the report and journal".into()); }
            }
            [command, path] if command == "recover-policy" => {
                let report = jack_windows_audio::policy::recover_policy_journal(std::path::Path::new(path))?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                if !report.complete { return Err("some routes remain pending recovery".into()); }
            }
            [command,pid,name,channel,report] if command == "sonar-set-app" => {
                use jack_core::routing::{AudioPolicy,Flow,Role,RouteKey};
                let pid:u32=pid.parse()?;
                let channel:jack_sonar::sonar::models::Channel=serde_json::from_value(serde_json::json!(channel))?;
                let mut policy=jack_windows_audio::policy::WindowsAudioPolicy::new()?;
                let binding=policy.bind_process(pid)?;
                let keys=[Role::Console,Role::Multimedia,Role::Communications].map(|role|RouteKey::App {stable_app_id:binding.clone(),flow:Flow::Render,role});
                let before=keys.iter().map(|key|policy.read(key)).collect::<Result<Vec<_>,_>>()?;
                let mut service=jack_sonar::Service::new()?;
                let initial=service.refresh(false);
                let requested=service.execute(jack_sonar::Command::Application {process_id:pid,process_name:name.clone(),channel});
                let actual=service.refresh(false);
                let after=keys.iter().map(|key|policy.read(key)).collect::<Result<Vec<_>,_>>()?;
                let value=serde_json::json!({"scope":"Direct GG application request for the user's specified app/target; no Jack UI or saved-rule claim","pid":pid,"channel":channel,"before_policy":before,"after_policy":after,"request_error":requested.err().map(|error|error.to_string()),"before_session":initial.applications.iter().find(|app|app.process_id==pid),"after_session":actual.applications.iter().find(|app|app.process_id==pid)});
                std::fs::write(report,serde_json::to_vec_pretty(&value)?)?;
                println!("{}",serde_json::to_string_pretty(&value)?);
            }
            [command,pid] if command == "policy-app-status" => {
                use jack_core::routing::{AudioPolicy, Flow, Role, RouteKey};
                let pid:u32=pid.parse()?;
                let mut policy = jack_windows_audio::policy::WindowsAudioPolicy::new()?;
                let binding=policy.bind_process(pid)?;
                let values=[Role::Console,Role::Multimedia,Role::Communications].map(|role| {
                    let key=RouteKey::App {stable_app_id:binding.clone(),flow:Flow::Render,role};
                    match policy.read(&key) {
                        Ok(endpoint)=>serde_json::json!({"role":role,"endpoint":endpoint}),
                        Err(error)=>serde_json::json!({"role":role,"error":error.to_string()}),
                    }
                });
                println!("{}",serde_json::to_string_pretty(&serde_json::json!({"read_only":true,"pid":pid,"binding":binding,"values":values}))?);
            }
            [command] if command == "policy-status" => {
                use jack_core::routing::{AudioPolicy, Flow, Role, RouteKey};
                let mut policy = jack_windows_audio::policy::WindowsAudioPolicy::new()?;
                let mut routes = vec![];
                for flow in [Flow::Render, Flow::Capture] {
                    for role in [Role::Console, Role::Multimedia, Role::Communications] {
                        let key = RouteKey::Default {flow, role};
                        let state = match policy.read(&key) {
                            Ok(endpoint) => serde_json::json!({"endpoint":endpoint,"key":key}),
                            Err(error) => serde_json::json!({"error":error.to_string(),"key":key}),
                        };
                        routes.push(state);
                    }
                }
                let identity = policy.bind_process(std::process::id())?;
                let mut app_routes = vec![];
                for role in [Role::Console, Role::Multimedia, Role::Communications] {
                    let key = RouteKey::App {stable_app_id:identity.clone(), flow:Flow::Render, role};
                    let state = match policy.read(&key) {
                        Ok(endpoint) => serde_json::json!({"endpoint":endpoint,"key":key}),
                        Err(error) => serde_json::json!({"error":error.to_string(),"key":key}),
                    };
                    app_routes.push(state);
                }
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({"capabilities":policy.capabilities(),"defaults":routes,"self_app_routes":app_routes}))?);
            }
            [command, endpoint] if command == "verify-events" => {
                let result = jack_windows_audio::verify_session_events(endpoint)?;
                println!("{}", serde_json::to_string_pretty(&result)?);
                if !result.passed { return Err("session event verification failed; see report".into()); }
            }
            [command, lab, source, output, seconds, gain] if command == "bridge" && lab == "--lab" => {
                println!("{}", serde_json::to_string_pretty(&jack_windows_audio::bridge_lab(source, output, seconds.parse()?, gain.parse()?)?)?);
            }
            [] => println!(
                "{}",
                serde_json::to_string_pretty(&jack_windows_audio::snapshot()?)?
            ),
            [command, seconds] if command == "watch" => {
                let seconds: u64 = seconds.parse()?;
                if !(1..=3600).contains(&seconds) {
                    return Err("watch duration must be 1..=3600 seconds".into());
                }
                let mut monitor = jack_windows_audio::AudioMonitor::new()?;
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
                while std::time::Instant::now() < deadline {
                    if monitor.take_changed() {
                        match monitor.snapshot() {
                            Ok(value) => println!("{}", serde_json::to_string(&value)?),
                            Err(error) => eprintln!("audio snapshot unavailable; retry scheduled: {error}"),
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
            _ => return Err("usage: jack-audio-probe [policy-status | watch SECONDS | verify-events RENDER_ID | verify-default-policy FLOW ID JOURNAL | verify-self-app-policy FLOW ID JOURNAL | recover-policy JOURNAL | recover-volume RECORD | verify-volume ID BACKUP | verify-pcm --lab RENDER_ID | verify-mixer --lab RENDER_ID | verify-bridge --lab SOURCE_ID OUTPUT_ID | verify-transport --lab SOURCE_ID OUTPUT_ID | verify-transport-long --lab SOURCE_ID OUTPUT_ID SECONDS | bridge --lab SOURCE_ID OUTPUT_ID SECONDS GAIN]".into()),
        }
    }
    #[cfg(not(windows))]
    return Err("jack-audio-probe requires Windows".into());
    Ok(())
}

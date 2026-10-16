use super::*;
use serde_json::json;
use sonar::{applications::parse_application_routing, discovery::parse_local_url};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

#[test]
fn discovery_rejects_external_hosts_credentials_and_alternate_loopback() {
    for address in [
        "https://example.com:1234",
        "http://127.0.0.2:1234",
        "http://user:password@localhost:1234",
        "ftp://127.0.0.1",
        "http://localhost:1234/#fragment",
        "http://localhost:1234/?redirect=https://example.com",
    ] {
        assert!(parse_local_url("http", address).is_err(), "{address}");
    }
    for address in [
        "127.0.0.1:1234",
        "http://localhost:1234",
        "http://[::1]:1234",
    ] {
        assert!(parse_local_url("http", address).is_ok());
    }
}

#[test]
fn failed_mutations_clear_connected_state_and_allow_recovery_reads_without_replaying() {
    for live in [false, true] {
        let mut service = Service::new().unwrap();
        let server = service.runtime.block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/mode"))
                .respond_with(ResponseTemplate::new(503))
                .expect(1)
                .mount(&server)
                .await;
            server
        });
        service.client = Some(SonarClient::new(url::Url::parse(&server.uri()).unwrap()).unwrap());
        service.state = State {
            connection: "connected".into(),
            mode: Some("classic".into()),
            volumes: vec![VolumeState {
                channel: MixerChannel::Game,
                volume: 0.4,
                muted: false,
            }],
            ..State::default()
        };
        let result = if live {
            service
                .set_volume_live(MixerChannel::Game, 0.2, 0.4)
                .map(|_| ())
        } else {
            service.execute(Command::Volume {
                channel: MixerChannel::Game,
                level: 0.2,
                expected: 0.4,
            })
        };
        assert!(result.is_err());
        assert!(service.client.is_none());
        assert!(!service.state.available());
        assert!(service.state.volumes.is_empty());
        assert!(service.state.message.is_some());
        assert!(service.next_retry <= Instant::now());
        let options = DiscoveryOptions {
            core_props: Some(std::env::temp_dir()),
        };
        assert_eq!(
            service.refresh_with_options(false, &options).connection,
            "disconnected"
        );
        let requests = service
            .runtime
            .block_on(server.received_requests())
            .unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "GET");
    }
}

#[test]
fn explicit_retry_bypasses_old_backoff_and_restarts_short_startup_retry() {
    let mut service = Service::new().unwrap();
    service.state.connection = "waiting".into();
    service.backoff = Duration::from_secs(30);
    service.next_retry = Instant::now() + Duration::from_secs(30);
    // This path is guaranteed not to be a file. Never discover the user's GG.
    let options = DiscoveryOptions {
        core_props: Some(std::env::temp_dir()),
    };
    assert_eq!(
        service.refresh_with_options(false, &options).connection,
        "waiting"
    );
    assert!(service.state.message.is_none());
    let state = service.refresh_with_options(true, &options);
    assert_eq!(state.connection, "disconnected");
    assert!(state.message.is_some());
    assert!(service.next_retry <= Instant::now() + Duration::from_secs(1));
    assert_eq!(service.backoff, Duration::from_secs(2));
}

#[test]
fn duplicate_virtual_targets_are_not_guessed() {
    let payload = json!([{"deviceId":"one","role":"game","dataFlow":"render"},{"deviceId":"two","role":"game","dataFlow":"render"}]);
    assert!(parse_application_routing(&payload).is_err());
}

#[test]
fn multiple_sessions_for_same_process_remain_ambiguous() {
    let session = json!({"processId":123,"processName":"browser","state":"active"});
    let payload = json!([{"deviceId":"one","role":"game","dataFlow":"render","audioSessions":[session]},{"deviceId":"two","role":"chatRender","dataFlow":"render","audioSessions":[session]}]);
    let result = parse_application_routing(&payload).unwrap();
    assert_eq!(result.sessions[0].route, ApplicationRoute::Multiple);
}

#[test]
fn unnamed_representative_keeps_metadata_from_another_current_session() {
    let payload = json!([
        {"deviceId":"one","role":"game","dataFlow":"render","audioSessions":[
            {"processId":123,"processName":"Unknown","state":"active"}]},
        {"deviceId":"physical","dataFlow":"render","audioSessions":[
            {"processId":123,"processName":"browser","displayName":"Browser","state":"active"},
            {"processId":123,"processName":"old","displayName":"Old name","state":"inactive"}]}
    ]);
    let result = parse_application_routing(&payload).unwrap();
    assert_eq!(result.sessions[0].process_name, "browser");
    assert_eq!(result.sessions[0].display_name, "Browser");
    assert_eq!(result.sessions[0].route, ApplicationRoute::Multiple);
}

#[test]
fn streamer_mode_clears_classic_state_and_blocks_writes() {
    let mut service = Service::new().unwrap();
    let server = service.runtime.block_on(async {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/mode"))
            .respond_with(ResponseTemplate::new(200).set_body_json("stream"))
            .mount(&server)
            .await;
        server
    });
    service.client = Some(SonarClient::new(url::Url::parse(&server.uri()).unwrap()).unwrap());
    let state = service.refresh(false);
    assert_eq!(state.connection, "unsupported_mode");
    assert!(state.volumes.is_empty());
    assert!(
        service
            .execute(Command::Volume {
                channel: MixerChannel::Game,
                level: 0.2,
                expected: 0.4
            })
            .is_err()
    );
    let requests = service
        .runtime
        .block_on(server.received_requests())
        .unwrap();
    assert!(requests.iter().all(|r| r.method == "GET"));
}

#[test]
fn successful_http_write_with_wrong_readback_is_an_error() {
    let service = Service::new().unwrap();
    service.runtime.block_on(async {
        let server = MockServer::start().await;
        let classic = json!({"volume":0.4,"muted":false});
        let volumes = json!({"masters":{"classic":classic},"devices":{"game":{"classic":classic},"chatRender":{"classic":classic},"chatCapture":{"classic":classic},"media":{"classic":classic},"aux":{"classic":classic}}});
        Mock::given(method("PUT")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        Mock::given(method("GET")).and(path("/volumeSettings/classic")).respond_with(ResponseTemplate::new(200).set_body_json(volumes)).mount(&server).await;
        let client = SonarClient::new(url::Url::parse(&server.uri()).unwrap()).unwrap();
        assert!(client.set_volume(MixerChannel::Game, 0.2).await.is_err());
        assert!(client.set_muted(MixerChannel::Game, true).await.is_err());
    });
}

#[test]
fn redirects_are_not_followed() {
    let service = Service::new().unwrap();
    service.runtime.block_on(async {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("Location", "https://example.com/private"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let client = SonarClient::new(url::Url::parse(&server.uri()).unwrap()).unwrap();
        assert!(client.volumes().await.is_err());
    });
}

#[test]
fn live_volume_uses_only_mode_and_volume_and_stops_on_external_changes() {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    let mut service = Service::new().unwrap();
    let value = Arc::new(AtomicU64::new(0.4f64.to_bits()));
    let server=service.runtime.block_on(async {
        let server=MockServer::start().await;
        Mock::given(method("GET")).and(path("/mode"))
            .respond_with(ResponseTemplate::new(200).set_body_json("classic")).mount(&server).await;
        let read=value.clone();
        Mock::given(method("GET")).and(path("/volumeSettings/classic"))
            .respond_with(move |_: &wiremock::Request| {
                let classic=json!({"volume":f64::from_bits(read.load(Ordering::SeqCst)),"muted":false});
                ResponseTemplate::new(200).set_body_json(json!({"masters":{"classic":classic},"devices":{"game":{"classic":classic},"chatRender":{"classic":classic},"chatCapture":{"classic":classic},"media":{"classic":classic},"aux":{"classic":classic}}}))
            }).mount(&server).await;
        let write=value.clone();
        Mock::given(method("PUT"))
            .respond_with(move |request: &wiremock::Request| {
                let level:f64=request.url.path().rsplit('/').next().unwrap().parse().unwrap();
                write.store(level.to_bits(),Ordering::SeqCst);ResponseTemplate::new(200)
            }).expect(1).mount(&server).await;
        server
    });
    service.client = Some(SonarClient::new(url::Url::parse(&server.uri()).unwrap()).unwrap());
    assert_eq!(
        service
            .set_volume_live(MixerChannel::Game, 0.3, 0.4)
            .unwrap(),
        0.3
    );
    value.store(0.7f64.to_bits(), Ordering::SeqCst);
    assert!(
        service
            .set_volume_live(MixerChannel::Game, 0.2, 0.3)
            .is_err()
    );
    let requests = service
        .runtime
        .block_on(server.received_requests())
        .unwrap();
    assert!(requests.iter().all(|r| r.url.path()=="/mode" || r.url.path().starts_with("/volumeSettings/classic")));
    assert_eq!(requests.iter().filter(|r| r.method == "PUT").count(), 1);
}

fn optional_source_fixture(enabled: &std::sync::atomic::AtomicBool) -> serde_json::Value {
    use std::sync::atomic::Ordering;
    json!([
        {"id":"game-source","role":"game","dataFlow":"render","isVad":true,"state":"active"},
        {"id":"aux-source","role":"aux","dataFlow":"render","isVad":true,"state":if enabled.load(Ordering::SeqCst) {"active"} else {"disabled"}}
    ])
}
fn optional_source_server(
    service: &Service,
    change_on_put: bool,
) -> (MockServer, std::sync::Arc<std::sync::atomic::AtomicBool>) {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    service.runtime.block_on(async {
        let server = MockServer::start().await;
        let active = Arc::new(AtomicBool::new(false));
        let classic=json!({"volume":0.4,"muted":false});
        let volumes=json!({"masters":{"classic":classic},"devices":{"game":{"classic":classic},"chatRender":{"classic":classic},"chatCapture":{"classic":classic},"media":{"classic":classic},"aux":{"classic":classic}}});
        for (endpoint,body) in [
            ("/mode",json!("classic")),
            ("/volumeSettings/classic",volumes),
            ("/classicRedirections",json!([{"id":"game","deviceId":"out"},{"id":"chat","deviceId":"out"},{"id":"media","deviceId":"out"},{"id":"aux","deviceId":"out"},{"id":"mic","deviceId":"mic"}])),
            ("/AudioDeviceRouting",json!([{"deviceId":"game-source","role":"game","dataFlow":"render","audioSessions":[]},{"deviceId":"aux-source","role":"aux","dataFlow":"render","audioSessions":[]}]))
        ] { Mock::given(method("GET")).and(path(endpoint)).respond_with(ResponseTemplate::new(200).set_body_json(body)).mount(&server).await; }
        let devices_active = active.clone();
        Mock::given(method("GET")).and(path("/audioDevices")).respond_with(move |_: &wiremock::Request| ResponseTemplate::new(200).set_body_json(optional_source_fixture(&devices_active))).mount(&server).await;
        let put_active = active.clone();
        Mock::given(method("PUT")).and(path("/audioInitialization/aux/isEnabled/true")).respond_with(move |_: &wiremock::Request| { if change_on_put {put_active.store(true,Ordering::SeqCst);} ResponseTemplate::new(200) }).expect(1).mount(&server).await;
        (server,active)
    })
}
#[test]
fn optional_source_activation_requires_active_source_readback_and_is_not_replayed() {
    let mut service = Service::new().unwrap();
    let (server, _) = optional_source_server(&service, true);
    service.client = Some(SonarClient::new(url::Url::parse(&server.uri()).unwrap()).unwrap());
    assert!(
        !service
            .refresh(false)
            .sources
            .iter()
            .any(|source| source.channel == Channel::Aux)
    );
    service
        .execute(Command::Source {
            channel: Channel::Aux,
            enabled: true,
            expected: false,
        })
        .unwrap();
    assert!(
        service
            .refresh(false)
            .sources
            .iter()
            .any(|source| source.channel == Channel::Aux)
    );
    service
        .execute(Command::Source {
            channel: Channel::Aux,
            enabled: true,
            expected: false,
        })
        .unwrap();
}
#[test]
fn optional_source_success_without_enabled_readback_is_rejected() {
    let mut service = Service::new().unwrap();
    let (server, _) = optional_source_server(&service, false);
    service.client = Some(SonarClient::new(url::Url::parse(&server.uri()).unwrap()).unwrap());
    assert!(
        service
            .execute(Command::Source {
                channel: Channel::Aux,
                enabled: true,
                expected: false
            })
            .is_err()
    );
}

#[test]
fn optional_source_deletion_waits_for_disabled_readback_and_does_not_replay() {
    use std::sync::atomic::Ordering;
    let mut service = Service::new().unwrap();
    let (server, active) = optional_source_server(&service, true);
    service.client = Some(SonarClient::new(url::Url::parse(&server.uri()).unwrap()).unwrap());
    service
        .execute(Command::Source {
            channel: Channel::Aux,
            enabled: true,
            expected: false,
        })
        .unwrap();
    service.runtime.block_on(async {
        Mock::given(method("PUT"))
            .and(path("/audioInitialization/aux/isEnabled/false"))
            .respond_with(move |_: &wiremock::Request| {
                active.store(false, Ordering::SeqCst);
                ResponseTemplate::new(200)
            })
            .expect(1)
            .mount(&server)
            .await;
    });
    for _ in 0..2 {
        service
            .execute(Command::Source {
                channel: Channel::Aux,
                enabled: false,
                expected: true,
            })
            .unwrap();
    }
    assert!(
        !service
            .refresh(false)
            .sources
            .iter()
            .any(|source| source.channel == Channel::Aux)
    );
}

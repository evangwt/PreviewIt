use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use previewit_broker::{InstanceError, InstanceLease, InstanceRole};
use uuid::Uuid;

fn unique_product_id(label: &str) -> String {
    format!("PreviewIt.Test.{label}.{}", Uuid::new_v4().simple())
}

#[test]
fn one_thread_owns_the_session_lease_and_a_contender_takes_over() {
    let product_id = unique_product_id("takeover");
    let owner_id = product_id.clone();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();

    let owner = thread::spawn(move || {
        let InstanceRole::Primary(lease) =
            InstanceLease::elect(&owner_id).expect("elect first owner")
        else {
            panic!("first election must own the lease");
        };
        ready_tx
            .send((lease.mutex_name().to_owned(), lease.pipe_name().to_owned()))
            .unwrap();
        release_rx.recv().unwrap();
        drop(lease);
    });

    let (mutex_name, pipe_name) = ready_rx.recv().unwrap();
    let mut contender = match InstanceLease::elect(&product_id).expect("elect contender") {
        InstanceRole::Secondary(contender) => contender,
        InstanceRole::Primary(_) => {
            release_tx.send(()).unwrap();
            owner.join().unwrap();
            panic!("second thread must not own the held lease");
        }
    };

    assert_eq!(contender.mutex_name(), mutex_name);
    assert_eq!(contender.pipe_name(), pipe_name);
    assert!(contender.try_take_over().unwrap().is_none());

    release_tx.send(()).unwrap();
    owner.join().unwrap();

    let deadline = Instant::now() + Duration::from_secs(1);
    let lease = loop {
        if let Some(lease) = contender.try_take_over().expect("retry takeover") {
            break lease;
        }
        assert!(Instant::now() < deadline, "released lease was not acquired");
        thread::yield_now();
    };
    assert_eq!(lease.mutex_name(), mutex_name);
    assert_eq!(lease.pipe_name(), pipe_name);
}

#[test]
fn election_names_are_session_local_and_deterministic() {
    let product_id = unique_product_id("names");
    let InstanceRole::Primary(lease) =
        InstanceLease::elect(&product_id).expect("elect named lease")
    else {
        panic!("unique product id must elect a primary");
    };

    assert!(lease.mutex_name().starts_with(r"Local\"));
    assert!(lease.mutex_name().contains(&product_id));
    assert!(!lease.pipe_name().contains(['\\', '/']));
    assert!(lease.pipe_name().contains(&product_id));
    let session_id = lease
        .pipe_name()
        .rsplit('.')
        .next()
        .expect("session suffix");
    assert!(session_id.parse::<u32>().is_ok());
    assert!(lease.mutex_name().ends_with(session_id));
}

#[test]
fn different_product_ids_have_independent_leases() {
    let first = InstanceLease::elect(&unique_product_id("first")).unwrap();
    let second = InstanceLease::elect(&unique_product_id("second")).unwrap();

    assert!(matches!(first, InstanceRole::Primary(_)));
    assert!(matches!(second, InstanceRole::Primary(_)));
}

#[test]
fn invalid_product_ids_are_rejected() {
    assert!(matches!(
        InstanceLease::elect(""),
        Err(InstanceError::InvalidProductId)
    ));
    assert!(matches!(
        InstanceLease::elect(r"PreviewIt\Global"),
        Err(InstanceError::InvalidProductId)
    ));
}

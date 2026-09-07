use communication_protocol::{AdmissionError, ControlAdmission};

#[test]
fn initialization_and_retired_ids_guard_dispatch() {
    let mut admission = ControlAdmission::default();
    assert_eq!(
        admission.admit("a", "endpoint/list"),
        Err(AdmissionError::NotInitialized)
    );
    assert!(admission.admit("init", "control/initialize").is_ok());
    assert_eq!(
        admission.admit("b", "endpoint/list"),
        Err(AdmissionError::NotInitialized)
    );
    admission.initialized();
    admission.complete("init");
    assert_eq!(
        admission.admit("again", "control/initialize"),
        Err(AdmissionError::AlreadyInitialized)
    );
    assert!(admission.admit("c", "endpoint/list").is_ok());
    admission.complete("c");
    assert_eq!(
        admission.admit("b", "endpoint/list"),
        Err(AdmissionError::ReusedId)
    );
}

#[test]
fn pending_limit_does_not_admit_additional_work() {
    let mut admission = ControlAdmission::default();
    assert!(admission.admit("init", "control/initialize").is_ok());
    admission.initialized();
    admission.complete("init");
    for index in 0..64 {
        assert!(
            admission
                .admit(&format!("request-{index}"), "endpoint/list")
                .is_ok()
        );
    }
    assert_eq!(
        admission.admit("overflow", "endpoint/list"),
        Err(AdmissionError::Overloaded)
    );
    admission.complete("request-0");
    assert!(admission.admit("next", "endpoint/list").is_ok());
}

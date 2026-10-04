//! Pairing a phone with this computer, from either side.

use super::*;

impl Workspace {
    pub fn phones(&self) -> &Phones {
        &self.phones
    }

    pub fn set_phones(&mut self, phones: Phones, cx: &mut Context<Self>) {
        self.phones = phones;
        cx.notify();
    }

    pub fn ask_phones(
        &mut self,
        request: PhonesRequest,
        cx: &mut Context<Self>,
    ) {
        cx.emit(WorkspaceEvent::Phones(request));
    }

    pub fn pairing(&self) -> &Pairing {
        &self.pairing
    }

    /// Replaces what pairing knows, as when the phone starts paired.
    pub fn set_pairing(&mut self, pairing: Pairing, cx: &mut Context<Self>) {
        self.pairing = pairing;
        cx.notify();
    }

    /// Opens pairing at `step`, with no way back to the runs until the
    /// phone reaches a computer.
    pub fn start_pairing(&mut self, step: PairStep, cx: &mut Context<Self>) {
        self.back_stack.clear();
        self.route = Route::Pair(step);
        self.entered(cx);
    }

    /// Records what the phone's remote learned: once paired, the phone
    /// is named; a computer that does not answer says so; one that
    /// answers again leads back to the runs.
    pub fn update_pairing(
        &mut self,
        update: PairingUpdate,
        cx: &mut Context<Self>,
    ) {
        let next = match &update {
            PairingUpdate::Paired(_) => Some(Route::Pair(PairStep::Paired)),
            PairingUpdate::Unreachable { .. } => {
                Some(Route::Pair(PairStep::Unreachable))
            }
            // Reached again: the runs, unless the phone just paired and
            // is being named.
            PairingUpdate::Connected(_) => {
                matches!(self.route, Route::Pair(PairStep::Unreachable))
                    .then_some(Route::Home)
            }
            PairingUpdate::Progress(_) | PairingUpdate::Unsent(_) => None,
        };
        self.pairing.update(update);
        match next {
            Some(route) => {
                self.back_stack.clear();
                self.route = route;
                self.entered(cx);
            }
            None => cx.notify(),
        }
    }

    /// Opens the camera to read a computer's pairing code.
    pub fn scan_pairing_code(&mut self, cx: &mut Context<Self>) {
        self.pairing.progress = Progress::Scanning;
        cx.emit(WorkspaceEvent::Pair(PairRequest::Scan));
        self.navigate(Route::Pair(PairStep::Scan), cx);
    }

    /// Pairs by typing the address and the code instead.
    pub fn type_address(&mut self, cx: &mut Context<Self>) {
        self.pairing.progress = Progress::Idle;
        self.navigate(Route::Pair(PairStep::Address), cx);
    }

    /// Connects to the typed address with the typed code.
    pub fn connect_typed(&mut self, cx: &mut Context<Self>) {
        if self.pairing.busy() {
            return;
        }
        let address =
            tau_remote::Address::typed(self.pair_address.read(cx).text());
        let secret =
            tau_remote::PairingSecret::typed(self.pair_code.read(cx).text());
        self.pairing.progress = match (address, secret) {
            (Ok(address), Ok(secret)) => {
                cx.emit(WorkspaceEvent::Pair(PairRequest::Typed {
                    address: address.clone(),
                    secret,
                }));
                Progress::Connecting { address }
            }
            (Err(error), _) | (_, Err(error)) => {
                Progress::Failed(error.to_string())
            }
        };
        cx.notify();
    }

    /// The person compared the certificate with the computer's, and it
    /// is the same: pair.
    pub fn trust_certificate(&mut self, cx: &mut Context<Self>) {
        if let Progress::Compare {
            address,
            fingerprint,
        } = &self.pairing.progress
        {
            cx.emit(WorkspaceEvent::Pair(PairRequest::Trust(*fingerprint)));
            self.pairing.progress = Progress::Pairing {
                address: address.clone(),
                fingerprint: *fingerprint,
            };
            cx.notify();
        }
    }

    /// The certificate is not the computer's: stop before sending the
    /// code.
    pub fn distrust_certificate(&mut self, cx: &mut Context<Self>) {
        cx.emit(WorkspaceEvent::Pair(PairRequest::Cancel));
        self.pairing.progress = Progress::Failed(
            "The certificate is not your computer's, so tau stopped before \
             sending the code. Check the address, or scan the code instead."
                .into(),
        );
        cx.notify();
    }

    /// Names the phone, if a name was typed, and goes to the runs.
    pub fn open_tau(&mut self, cx: &mut Context<Self>) {
        let name = self.phone_name.read(cx).text().trim().to_owned();
        if !name.is_empty() {
            cx.emit(WorkspaceEvent::Pair(PairRequest::Name(name)));
        }
        self.back_stack.clear();
        self.route = Route::Home;
        self.entered(cx);
    }

    /// Connects to the paired computer again.
    pub fn retry_connection(&mut self, cx: &mut Context<Self>) {
        if self.pairing.busy() {
            return;
        }
        if let Some(computer) = &self.pairing.computer {
            self.pairing.progress = Progress::Connecting {
                address: computer.address.clone(),
            };
        }
        cx.emit(WorkspaceEvent::Pair(PairRequest::Retry));
        cx.notify();
    }

    /// The phone's own name, offered once paired.
    pub fn set_phone_name(&mut self, name: String, cx: &mut Context<Self>) {
        self.phone_name
            .update(cx, |input, cx| input.set_text(name, cx));
    }

    /// Fills the pairing fields, as typing would: for tests.
    pub fn pair_fields_for_test(
        &mut self,
        address: &str,
        code: &str,
        cx: &mut Context<Self>,
    ) {
        self.pair_address
            .update(cx, |input, cx| input.set_text(address.to_owned(), cx));
        self.pair_code
            .update(cx, |input, cx| input.set_text(code.to_owned(), cx));
    }
}

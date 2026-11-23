//! Signing in: QR code, token, email and password, and the session that follows.

use super::*;
use iced::widget::column;

impl Gui {
    pub(super) fn start_session(&mut self, token: String) -> Task<Message> {
        match crate::session::client(&token) {
            Ok(r) => self.rest = Some(r),
            Err(e) => {
                self.login.error = Some(e);
                return Task::none();
            }
        }
        self.token = Some(token);
        self.session += 1;
        self.conn = Conn::Connecting;
        self.screen = Screen::Main;
        Task::none()
    }

    pub(super) fn finish_login(&mut self, token: String, user: Option<User>) -> Task<Message> {
        if let Some(warning) = crate::session::save_token(&token) {
            self.status = Some((warning, true));
        }
        let task = self.start_session(token);
        if let Some(u) = user {
            self.store.me = Some(u);
        }
        task
    }

    pub(super) fn logout(&mut self, notice: &str) {
        config::delete_token();
        self.leave_voice();
        let session = self.session + 1;
        let attempt = self.login.attempt + 1;
        let voice_attempt = self.voice_attempt;
        *self = Self::blank();
        self.voice_attempt = voice_attempt;
        self.session = session;
        self.login.attempt = attempt;
        self.login.notice = Some(notice.to_string());
    }

    pub(super) fn on_auth(&mut self, ev: AuthEvent) -> Task<Message> {
        if self.screen != Screen::Login {
            return Task::none();
        }
        match ev {
            AuthEvent::Progress(s) => self.login.qr = QrView::Working(s),
            AuthEvent::Qr(url) => {
                self.login.qr = match qr_code::Data::new(url) {
                    Ok(data) => QrView::Ready(data),
                    Err(_) => QrView::Failed(
                        t(
                            "Could not generate the QR code",
                            "Impossibile generare il codice QR",
                        )
                        .into(),
                    ),
                }
            }
            AuthEvent::Scanned(user) => self.login.qr = QrView::Scanned(user),
            AuthEvent::Token(token) => return self.finish_login(token, None),
            AuthEvent::Failed(e) => self.login.qr = QrView::Failed(e),
        }
        Task::none()
    }

    pub(super) fn submit_token(&mut self) -> Task<Message> {
        if self.login.busy {
            return Task::none();
        }
        let (token, client) = match crate::session::prepare_token(&self.login.token) {
            Ok(v) => v,
            Err(e) => {
                self.login.error = Some(e);
                return Task::none();
            }
        };
        self.login.busy = true;
        self.login.error = None;
        Task::perform(crate::session::verify_token(client, token), Message::TokenChecked)
    }

    pub(super) fn submit_password(&mut self) -> Task<Message> {
        if self.login.busy {
            return Task::none();
        }
        let email = match crate::session::credentials(&self.login.email, &self.login.password) {
            Ok(e) => e,
            Err(e) => {
                self.login.error = Some(e);
                return Task::none();
            }
        };
        let password = self.login.password.clone();
        self.login.busy = true;
        self.login.error = None;
        self.login.notice = None;
        Task::perform(
            async move { password_login::login(&email, &password).await },
            Message::PasswordAnswer,
        )
    }

    pub(super) fn submit_code(&mut self) -> Task<Message> {
        if self.login.busy {
            return Task::none();
        }
        let Some(challenge) = self.login.two_factor.clone() else {
            return Task::none();
        };
        let code = self.login.code.trim().to_string();
        if code.is_empty() {
            return Task::none();
        }
        self.login.busy = true;
        self.login.error = None;
        Task::perform(
            async move { password_login::two_factor(&challenge, &code).await },
            Message::PasswordAnswer,
        )
    }

    pub(super) fn on_password_answer(&mut self, result: Result<Answer, String>) -> Task<Message> {
        let login = &mut self.login;
        login.busy = false;
        let answer = match result {
            Ok(a) => a,
            // No answer: the fields stay as they are, to try again.
            Err(e) => {
                login.error = Some(e);
                return Task::none();
            }
        };
        // Whatever the answer, the password is no longer needed.
        login.password.clear();
        login.code.clear();
        match crate::session::next_step(answer) {
            Next::SignedIn(token) => {
                login.two_factor = None;
                login.error = None;
                return self.finish_login(token, None);
            }
            Next::AskCode(m) => {
                login.two_factor = Some(m);
                login.error = None;
            }
            Next::Notice(notice) => {
                login.two_factor = None;
                login.error = None;
                login.notice = Some(notice);
            }
            Next::Error { message, forget_code } => {
                if forget_code {
                    login.two_factor = None;
                }
                login.error = Some(message);
            }
        }
        Task::none()
    }

    pub(super) fn view_login(&self) -> Element<'_, Message> {
        let tabs = row![
            button(text("QR code").font(SEMIBOLD))
                .on_press(Message::LoginTab(LoginTab::Qr))
                .padding([6, 14])
                .style(tab(self.login.tab == LoginTab::Qr)),
            button(text(t("Email", "Email")).font(SEMIBOLD))
                .on_press(Message::LoginTab(LoginTab::Password))
                .padding([6, 14])
                .style(tab(self.login.tab == LoginTab::Password)),
            button(text("Token").font(SEMIBOLD))
                .on_press(Message::LoginTab(LoginTab::Token))
                .padding([6, 14])
                .style(tab(self.login.tab == LoginTab::Token)),
        ]
        .spacing(8);

        let body: Element<'_, Message> = match self.login.tab {
            LoginTab::Qr => match &self.login.qr {
                QrView::Working(msg) => text(msg.clone()).color(pal::muted()).into(),
                QrView::Ready(data) => column![
                    container(qr_code(data).cell_size(5).style(|_| qr_code::Style {
                        cell: iced::Color::BLACK,
                        background: iced::Color::WHITE,
                    }))
                    .padding(12)
                    .style(rounded(iced::Color::WHITE, 10.0)),
                    text(t(
                        "Scan the code with the Discord app on your phone",
                        "Inquadra il codice con l'app Discord sul telefono"
                    )),
                    text(t("Settings → Scan QR Code", "Impostazioni → Scansiona codice QR"))
                        .size(12)
                        .color(pal::muted()),
                ]
                .spacing(10)
                .align_x(Horizontal::Center)
                .into(),
                QrView::Scanned(user) => column![
                    text(tf!("✓ Code scanned by {user}", "✓ Codice scansionato da {user}"))
                        .color(pal::green()),
                    text(t(
                        "Confirm the login on your phone…",
                        "Conferma l'accesso sul telefono…"
                    ))
                    .color(pal::muted()),
                ]
                .spacing(8)
                .align_x(Horizontal::Center)
                .into(),
                QrView::Failed(err) => column![
                    text(err.clone()).color(pal::red()),
                    button(text(t("New code", "Nuovo codice")))
                        .on_press(Message::RetryQr)
                        .padding([6, 14])
                        .style(ghost),
                    text(t(
                        "Or sign in with email and password, or with the token.",
                        "Oppure accedi con email e password, o con il token."
                    ))
                    .size(12)
                    .color(pal::muted()),
                ]
                .spacing(10)
                .align_x(Horizontal::Center)
                .into(),
            },
            LoginTab::Password => self.view_password_login(),
            LoginTab::Token => {
                let path = config::token_location();
                let mut col = column![
                    text(t(
                        "Paste your account's token.",
                        "Incolla il token del tuo account."
                    )),
                    text_input("Token", &self.login.token)
                        .secure(true)
                        .on_input(Message::TokenInput)
                        .on_submit(Message::SubmitToken)
                        .padding(10)
                        .style(input),
                    button(
                        text(if self.login.busy {
                            t("Checking…", "Verifica…")
                        } else {
                            t("Sign in", "Accedi")
                        })
                        .font(SEMIBOLD)
                    )
                    .on_press_maybe((!self.login.busy).then_some(Message::SubmitToken))
                    .padding([8, 18])
                    .style(primary),
                    text(tf!(
                        "It will be saved in {path}, protected by the system.",
                        "Verrà salvato in {path}, protetto dal sistema."
                    ))
                    .size(11)
                    .color(pal::muted()),
                ]
                .spacing(12)
                .align_x(Horizontal::Center);
                if let Some(e) = &self.login.error {
                    col = col.push(text(e.clone()).color(pal::red()));
                }
                col.into()
            }
        };

        let language = button(text(tf!("Language: {}", "Lingua: {}", i18n::lang().name())).size(12))
            .on_press(Message::ToggleLanguage)
            .padding([3, 10])
            .style(ghost);
        let mut card = column![text("Rico").size(28).font(BOLD), tabs]
            .spacing(18)
            .align_x(Horizontal::Center)
            .width(420);
        if let Some(n) = &self.login.notice {
            card = card.push(text(n.clone()).color(pal::yellow()));
        }
        card = card
            .push(container(body).width(Length::Fill).align_x(Horizontal::Center))
            .push(
                text(t(
                    "Unofficial client: using it may violate Discord's Terms of Service.",
                    "Client non ufficiale: usarlo può violare i Termini di Servizio di Discord.",
                ))
                .size(11)
                .color(pal::muted()),
            )
            .push(language);

        container(container(card).padding(28).style(style::card))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center)
            .into()
    }

    pub(super) fn view_password_login(&self) -> Element<'_, Message> {
        let login = &self.login;
        let sign_in = button(
            text(if login.busy {
                t("Signing in…", "Accesso in corso…")
            } else {
                t("Sign in", "Accedi")
            })
            .font(SEMIBOLD),
        )
        .padding([8, 18])
        .style(primary);
        let mut col = if login.two_factor.is_some() {
            column![
                text(t(
                    "Write the code of your authenticator app.",
                    "Scrivi il codice della tua app di autenticazione."
                )),
                text_input(t("Code", "Codice"), &login.code)
                    .on_input(Message::CodeInput)
                    .on_submit(Message::SubmitCode)
                    .padding(10)
                    .style(input),
                row![
                    sign_in.on_press_maybe((!login.busy).then_some(Message::SubmitCode)),
                    button(text(t("Back", "Indietro")))
                        .on_press(Message::CancelTwoFactor)
                        .padding([8, 14])
                        .style(ghost),
                ]
                .spacing(10),
            ]
        } else {
            column![
                text(t(
                    "Sign in with your Discord account's email (or phone) and password.",
                    "Accedi con l'email (o il telefono) e la password del tuo account Discord."
                )),
                text_input(t("Email or phone", "Email o telefono"), &login.email)
                    .on_input(Message::EmailInput)
                    .on_submit(Message::SubmitPassword)
                    .padding(10)
                    .style(input),
                text_input("Password", &login.password)
                    .secure(true)
                    .on_input(Message::PasswordInput)
                    .on_submit(Message::SubmitPassword)
                    .padding(10)
                    .style(input),
                sign_in.on_press_maybe((!login.busy).then_some(Message::SubmitPassword)),
                text(t(
                    "The password goes only to Discord and is not saved: Rico keeps only the token, protected by the system.",
                    "La password va solo a Discord e non viene salvata: Rico conserva solo il token, protetto dal sistema."
                ))
                .size(11)
                .color(pal::muted()),
            ]
        }
        .spacing(12)
        .align_x(Horizontal::Center);
        if let Some(e) = &login.error {
            col = col.push(text(e.clone()).color(pal::red()));
        }
        col.into()
    }
}

impl Gui {
    /// The login messages of `Gui::handle`.
    pub(super) fn handle_login(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::LoginTab(tab) => {
                self.login.tab = tab;
                self.login.error = None;
            }
            Message::RetryQr => {
                self.login.attempt += 1;
                self.login.qr = QrView::Working(t("Starting…", "Avvio…").into());
            }
            Message::Auth(ev) => return self.on_auth(ev),
            Message::TokenInput(s) => {
                self.login.token = s;
                self.login.error = None;
            }
            Message::SubmitToken => return self.submit_token(),
            Message::TokenChecked(result) => {
                self.login.busy = false;
                match result {
                    Ok((token, user)) => {
                        self.login.token.clear();
                        return self.finish_login(token, Some(user));
                    }
                    Err(e) => self.login.error = Some(e),
                }
            }
            Message::EmailInput(s) => {
                self.login.email = s;
                self.login.error = None;
            }
            Message::PasswordInput(s) => {
                self.login.password = s;
                self.login.error = None;
            }
            Message::SubmitPassword => return self.submit_password(),
            Message::CodeInput(s) => {
                // Six digits: nothing longer is needed.
                self.login.code = s.chars().take(12).collect();
                self.login.error = None;
            }
            Message::SubmitCode => return self.submit_code(),
            Message::CancelTwoFactor => {
                self.login.two_factor = None;
                self.login.code.clear();
                self.login.error = None;
            }
            Message::PasswordAnswer(result) => return self.on_password_answer(result),
            Message::Logout => self.logout(t("You signed out.", "Sei uscito dall'account.")),
            _ => {}
        }
        Task::none()
    }
}

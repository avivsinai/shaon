use shaon::config::Config;

/// Verify that Config rejects a TOML blob missing required fields.
#[test]
fn config_requires_mandatory_fields() {
    let bad_toml = r#"subdomain = "demo""#; // missing username
    let result: Result<Config, _> = toml::from_str(bad_toml);
    assert!(result.is_err(), "Config should reject incomplete TOML");
}

/// Verify that Config parses a complete TOML blob with all required fields.
#[test]
fn config_parses_valid_toml() {
    let good_toml = r#"
        subdomain = "demo"
        username = "user123"
        password = "pass456"
    "#;
    let config: Config = toml::from_str(good_toml).expect("should parse valid config");
    assert_eq!(config.subdomain, "demo");
    assert_eq!(config.username, "user123");
    assert_eq!(config.password.as_deref(), Some("pass456"));
    assert_eq!(config.payslip_fmt(), "%Y-%m.pdf");
}

/// Verify that optional payslip_format overrides the default.
#[test]
fn config_custom_payslip_format() {
    let toml_str = r#"
        subdomain = "demo"
        username = "user123"
        password = "pass456"
        payslip_format = "payslip-%Y-%m.pdf"
    "#;
    let config: Config = toml::from_str(toml_str).expect("should parse config");
    assert_eq!(config.payslip_fmt(), "payslip-%Y-%m.pdf");
}

/// Verify that parse_aspx_form_fields extracts hidden inputs.
#[test]
fn parse_aspx_form_fields_extracts_hidden_inputs() {
    let html = r#"
        <html><body>
        <form id="aspnetForm">
            <input type="hidden" name="__VIEWSTATE" value="abc123" />
            <input type="hidden" name="__EVENTVALIDATION" value="xyz789" />
            <input type="text" name="username" value="test" />
            <input type="submit" name="btnSubmit" value="Submit" />
        </form>
        </body></html>
    "#;
    let fields = shaon::client::parse_aspx_form_fields(html);
    assert_eq!(
        fields.get("__VIEWSTATE").map(String::as_str),
        Some("abc123")
    );
    assert_eq!(
        fields.get("__EVENTVALIDATION").map(String::as_str),
        Some("xyz789")
    );
    assert_eq!(fields.get("username").map(String::as_str), Some("test"));
    // Submit buttons should NOT be captured
    assert!(!fields.contains_key("btnSubmit"));
}

/// Verify that select elements are parsed.
#[test]
fn parse_aspx_form_fields_extracts_select() {
    let html = r#"
        <html><body>
        <form id="aspnetForm">
            <select name="dropdown">
                <option value="a">A</option>
                <option value="b" selected>B</option>
                <option value="c">C</option>
            </select>
        </form>
        </body></html>
    "#;
    let fields = shaon::client::parse_aspx_form_fields(html);
    assert_eq!(fields.get("dropdown").map(String::as_str), Some("b"));
}

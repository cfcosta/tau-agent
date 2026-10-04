//! Scripts as the cards show them: formatted when they parse, as they
//! came when they do not.

use tau_codemode::format::formatted;

#[test]
fn a_one_line_script_reads_as_code() {
    let code = "local n: number = 1 for i=1,3 do n+=i end return {total=n}";
    assert_eq!(
        &*formatted(code),
        "local n: number = 1\n\
         for i = 1, 3 do\n  n += i\nend\n\
         return { total = n }"
    );
}

/// The options line is a comment, and stays on top.
#[test]
fn the_options_line_stays() {
    let code = "-- @options: {\"timeout\": 5}\nreturn  1";
    assert_eq!(
        &*formatted(code),
        "-- @options: {\"timeout\": 5}\nreturn 1"
    );
}

/// A script still streaming in does not parse; it shows as it is.
#[test]
fn a_partial_script_is_left_alone() {
    let code = "local x = tools.read({ path =";
    assert_eq!(&*formatted(code), code);
}

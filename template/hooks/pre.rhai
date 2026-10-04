// Checks the name again for inputs the init hook cannot see, fixes the license year and
// file name, and removes the hooks from the generated project.
import "hooks/name_rules" as rules;

let name = variable::get("project-name");
if !rules::is_valid(name) {
    // By now cargo-generate has created the project's directory, unless it generates into
    // the current one (--init).
    let init = variable::is_set("is_init") && variable::get("is_init");
    let hint = if init { "" } else { `; remove the directory '${name}' that was created for it` };
    abort(`${rules::message(name)}${hint}`);
}

if !variable::is_set("year") {
    let today = system::date();
    variable::set("year", `${today.year}`);
}

let license = variable::get("license");
if license == "MIT" {
    file::rename("LICENSE-MIT", "LICENSE");
}
if license == "Apache-2.0" {
    file::rename("LICENSE-APACHE", "LICENSE");
}

file::delete("hooks");

// Checks the name again for inputs the init hook cannot see, fixes the license year and
// file name, and removes the hooks from the generated project.
import "hooks/name_rules" as rules;

let name = variable::get("project-name");
if !rules::is_valid(name) {
    abort(rules::message(name));
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

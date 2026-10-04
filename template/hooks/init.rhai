// Takes over the project name prompt, so a name typed at the prompt follows the same rules
// as one given with --name, and a rejected name never leaves a directory behind.
import "hooks/name_rules" as rules;

let raw = variable::get("project-name");
if raw == "" {
    try {
        loop {
            let name = variable::prompt("Project name");
            if rules::is_valid(name) {
                variable::set("project-name", name);
                break;
            }
            print(rules::message(name));
        }
    } catch (err) {
        // No terminal: cargo-generate takes the name from its other sources, such as
        // CARGO_GENERATE_VALUE_PROJECT_NAME, and the pre hook checks it.
    }
} else if !rules::is_valid(raw) {
    abort(rules::message(raw));
}

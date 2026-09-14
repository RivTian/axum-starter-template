// Validate final project identities before rendering and derive the environment prefix.
fn valid_identity(value, max_len) {
    if value.len() == 0 || value.len() > max_len {
        return false;
    }
    if !"abcdefghijklmnopqrstuvwxyz".contains(value.sub_string(0, 1)) {
        return false;
    }
    for part in value.split("-") {
        if part.len() == 0 {
            return false;
        }
        let i = 0;
        while i < part.len() {
            if !"abcdefghijklmnopqrstuvwxyz0123456789".contains(part.sub_string(i, 1)) {
                return false;
            }
            i += 1;
        }
    }
    return true;
}
let prefix = variable::get("crate_prefix");
if !valid_identity(prefix, 32) {
    abort("crate_prefix must be lowercase kebab-case, at most 32 characters");
}
let project = variable::get("project-name");
if !valid_identity(project, 64) {
    abort("project-name must be lowercase kebab-case, at most 64 characters");
}
let crate_name = variable::get("crate_name");
variable::set("env_prefix", crate_name.to_upper());

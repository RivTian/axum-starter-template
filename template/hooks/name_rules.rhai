// Project name rules, shared by the init and pre hooks.

fn reserved_names() {
    [
        "build", "deps", "examples", "incremental",
        "con", "prn", "aux", "nul",
        "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9",
        "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ]
}

// 2 to 64 characters: a lowercase letter, then lowercase letters, digits and single
// hyphens, not ending with a hyphen.
fn is_well_formed(name) {
    let count = name.len();
    if count < 2 || count > 64 {
        return false;
    }
    let first = true;
    let previous = ' ';
    for c in name.chars() {
        if first {
            if c < 'a' || c > 'z' {
                return false;
            }
            first = false;
        } else if c == '-' {
            if previous == '-' {
                return false;
            }
        } else if !((c >= 'a' && c <= 'z') || (c >= '0' && c <= '9')) {
            return false;
        }
        previous = c;
    }
    previous != '-'
}

fn is_reserved(name) {
    reserved_names().contains(name)
}

fn is_valid(name) {
    is_well_formed(name) && !is_reserved(name)
}

fn message(name) {
    if is_well_formed(name) {
        `invalid project name '${name}': the name is reserved`
    } else {
        `invalid project name '${name}': use 2 to 64 characters: lowercase letters, digits and single hyphens, starting with a letter`
    }
}

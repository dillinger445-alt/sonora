# Turns a saved `cargo test` or `cargo clippy` log into GitHub annotations
# (`::error` lines), so failures show in the run summary (and through the
# API) without opening the raw log: failing tests with their output, and
# compiler, lint and linker errors with their locations.
# GitHub keeps at most 10 error annotations per step.

# Message text: escape %, drop carriage returns.
function msg(s) { sub(/\r$/, "", s); gsub(/%/, "%25", s); return s }
# Property values (the title) also need : and , escaped.
function prop(s) { s = msg(s); gsub(/:/, "%3A", s); gsub(/,/, "%2C", s); return s }
function emit(title, text) {
  if (count >= 10) return
  printf "::error title=%s::%s\n", prop(title), text
  count++
}

{ line = msg($0) }

# A compiler error: report it with its location (the next line) when present.
pending != "" {
  if (line ~ /^ *--> /) { emit("Error", pending "%0A" line); pending = ""; next }
  emit("Error", pending)
  pending = ""
}

# A failing test: its captured output, up to the first blank line after it.
/^---- .* stdout ----$/ { name = $2; body = ""; inblock = 1; next }
inblock {
  if (line == "") {
    if (body != "") { emit("Failing test " name, body); inblock = 0 }
    next
  }
  if (length(body) < 4000) body = (body == "" ? line : body "%0A" line)
  next
}

/^error(\[E[0-9]+\])?: / && line !~ /^error: (test failed|could not compile|[0-9]+ target)/ { pending = line; next }
/LNK[0-9]+|undefined reference|ld: library not found|cannot find -l/ { emit("Linker error", line) }

END { if (pending != "") emit("Error", pending) }

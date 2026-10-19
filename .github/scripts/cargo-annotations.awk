# Turns a saved `cargo test`, `cargo clippy` or `tauri build` log into GitHub
# annotations (`::error` lines), so failures show in the run summary (and
# through the API) without opening the raw log: failing tests with their
# panic message, compiler, lint and linker errors with their locations, and
# Tauri's own errors. When nothing matches, the last lines of the log are
# posted instead. GitHub keeps at most 10 error annotations per step.

# Message text: escape %, drop colour codes and carriage returns.
function msg(s) {
  gsub(/\033\[[0-9;]*[A-Za-z]/, "", s)
  sub(/\r$/, "", s)
  gsub(/%/, "%25", s)
  return s
}
# Property values (the title) also need : and , escaped.
function prop(s) { gsub(/:/, "%3A", s); gsub(/,/, "%2C", s); return s }
function emit(title, text) {
  if (count >= 10) return
  printf "::error title=%s::%s\n", prop(title), text
  count++
}
# A failing test's captured output: from the panic on (or its last lines).
function flush(   i, start, text, n) {
  if (!inblock) return
  inblock = 0
  start = 0
  for (i = 1; i <= nb; i++) if (block[i] ~ /panicked at/) { start = i; break }
  if (!start) start = (nb > 20 ? nb - 19 : 1)
  text = ""
  n = 0
  for (i = start; i <= nb && n < 30; i++) {
    if (block[i] == "" && text == "") continue
    text = (text == "" ? block[i] : text "%0A" block[i])
    n++
  }
  sub(/(%0A)+$/, "", text)
  emit("Failing test " name, text)
}

{ line = msg($0); tail[++lines % 40] = line }

# A compiler or lint error: report it with its location (the next line).
pending != "" {
  if (line ~ /^ *--> /) { emit("Error", pending "%0A" line); pending = ""; next }
  emit("Error", pending)
  pending = ""
}

line ~ /^---- .* stdout ----$/ { flush(); split(line, w, " "); name = w[2]; nb = 0; inblock = 1; next }
inblock && (line ~ /^failures:$/ || line ~ /^test result:/) { flush(); next }
inblock { block[++nb] = line; next }

line ~ /^error(\[E[0-9]+\])?: / && line !~ /^error: (test failed|could not compile|[0-9]+ target)/ { pending = line; next }
line ~ /LNK[0-9]+|undefined reference|ld: library not found|cannot find -l/ { emit("Linker error", line) }
line ~ /^ *(Error|error) +(failed|[A-Z])/ || line ~ /failed to (bundle|build|run|sign|notarize)/ { emit("Build error", line) }

END {
  flush()
  if (pending != "") emit("Error", pending)
  if (count == 0) {
    text = ""
    for (i = (lines > 40 ? lines - 39 : 1); i <= lines; i++) text = (text == "" ? tail[i % 40] : text "%0A" tail[i % 40])
    emit("Last lines of the log", text)
  }
}

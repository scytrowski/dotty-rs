ensure_checkout() {
  local name=$1
  local repository=$2
  local tag=$3
  local revision=$4
  local checkout="${cache_dir}/${name}-${tag}"
  if [[ ! -e "$checkout" ]]; then
    git clone --depth 1 --branch "$tag" "$repository" "$checkout"
  fi
  local actual
  actual=$(git -C "$checkout" rev-parse HEAD 2>/dev/null || true)
  if [[ "$actual" != "$revision" ]]; then
    echo "$name checkout revision mismatch: expected $revision, got ${actual:-unknown} at $checkout" >&2
    return 1
  fi
  printf '%s\n' "$checkout"
}

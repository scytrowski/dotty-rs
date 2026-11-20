-- Optional interoperability check with a real Neovim client.
local dir = vim.fn.tempname()

local function run()
  local binary = assert(vim.env.DOTTY_LSP_BIN, "set DOTTY_LSP_BIN to the fixture executable")
  assert(vim.fn.mkdir(dir) == 1)
  local function open(name, method)
    local path = dir .. "/" .. name .. ".scala"
    assert(vim.fn.writefile({"class " .. name .. " {", method, "}"}, path) == 0)
    vim.cmd.edit(vim.fn.fnameescape(path))
    vim.bo.filetype = "scala"
    return vim.api.nvim_get_current_buf()
  end
  local a = open("A", "  def value(): Int = 42")
  local caller = "  def read(a: A): Int = a.value()"
  local b = open("B", caller)
  local id = assert(vim.lsp.start({name = "dotty-lsp", cmd = {binary}, root_dir = dir}))
  assert(vim.lsp.buf_attach_client(a, id))
  local client = assert(vim.lsp.get_client_by_id(id))
  assert(vim.wait(10000, function()
    return client.initialized and vim.lsp.buf_is_attached(a, id) and vim.lsp.buf_is_attached(b, id)
  end), "initialize and attach both Scala buffers")

  vim.api.nvim_win_set_cursor(0, {2, assert(caller:find("value", 1, true)) - 1})
  vim.lsp.buf.definition()
  assert(vim.wait(10000, function()
    return vim.api.nvim_get_current_buf() == a
  end), "definition must open A.scala")
  assert(vim.deep_equal(vim.api.nvim_win_get_cursor(0), {2, 6}), "cursor must land on value")

  client.stop()
  assert(vim.wait(10000, function() return client.is_stopped() end), "server shutdown")
end

local ok, err = xpcall(run, debug.traceback)
vim.fn.delete(dir, "rf")
if not ok then
  io.stderr:write(err .. "\n")
  vim.cmd("cquit 1")
end
print("Neovim jumped to A.value")
vim.cmd("qa!")

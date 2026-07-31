--- Drop the heading and the standalone-reading boilerplate `CHANGELOG.md`
--- carries, which the page it is included into supplies itself as a title, a
--- subtitle and a description, and which would otherwise give the page a second
--- level-one heading. Everything from the first version heading onward is kept
--- as written.
--- @param doc pandoc.Pandoc The whole document, preamble included
--- @return pandoc.Pandoc
function Pandoc(doc)
  local kept = pandoc.List({})
  local started = false
  for _, block in ipairs(doc.blocks) do
    started = started or (block.t == 'Header' and block.level == 2)
    if started then
      kept:insert(block)
    end
  end
  doc.blocks = kept
  return doc
end

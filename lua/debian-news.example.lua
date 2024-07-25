-- A Lua extractor script is first run when Feedgen starts. The script has
-- access to a number of standard APIs:
-- - coroutine
-- - table
-- - string
-- - utf8
-- - math
--
-- In addition to these, Feedgen exports a global variable `feedgen` that
-- contains the Feedgen API:
--
-- - `feedgen.parseSelector`: parses a string as a CSS selector (more below).
-- - `feedgen.parseHtml`: parses a source buffer as an HTML document (more
--   below).
--
-- - `feedgen.log`: a table of logging functions:
--   - `feedgen.log.trace`: logs a message at the TRACE level.
--   - `feedgen.log.debug`: logs a message at the DEBUG level.
--   - `feedgen.log.info`: logs a message at the INFO level.
--   - `feedgen.log.warn`: logs a message at the WARN level.
--   - `feedgen.log.error`: logs a message at the ERROR level.
--
--   All of these functions are variadic; arguments are converted to strings as
--   if by calls to `tostring` and joined together with a space character.
--
-- - `print` and `warn` log messages at the INFO and WARN levels respectively.

local months = {
  Jan = 1,
  Feb = 2,
  Mar = 3,
  Apr = 4,
  May = 5,
  Jun = 6,
  Jul = 7,
  Aug = 8,
  Sep = 9,
  Oct = 10,
  Nov = 11,
  Dec = 12,
}

-- The extractor script must export a **global** function named `extract`.
function extract(source)
  -- This function is called every time a feed is updated to extract feed
  -- entries from the retrieved source page.
  --
  -- The function is provided a single argument of type `Source`, which is a
  -- reference-counted handle to the buffer with the source page's contents.
  -- The `__len` and `__tostring` metamethods are defined for the `Source`,
  -- meaning you can use `#source` to get the length of the contents and
  -- `tostring(source)` to load the contents into the VM as a Lua string (not
  -- recommended: they can be huge).

  -- The main use for the source is to pass it directly to `feedgen.parseHtml`.
  -- It parses the source (or a plain string) as an HTML document, and does so
  -- quite permissively: you'll always get a valid HTML document regardless of
  -- what nonsense you feed to the function, so it's quite robust even with
  -- broken websites (and the majority of websites are such).
  local html = feedgen.parseHtml(source)

  -- `feedgen.parseHtml` returns a handle to the HTML DOM. The handle has two
  -- methods:
  -- - `html:select`: selects elements matching a CSS selector (see below).
  -- - `html:root`: returns a reference to the root element (`<html>`).
  --
  -- Note that the DOM is kept in memory as long as a reference to any DOM node
  -- is alive. So if you save references outside the `extract` function, you'll
  -- get a memory leak.

  -- Element lookup is usually performed with the use of CSS selectors by
  -- calling `:select`. The argument can be a plain string, in which case it is
  -- first parsed as a CSS selector (potentially throwing an error), or a
  -- precompiled `Selector`, obtained by calling `html.parseSelector`. It's a
  -- good idea to precompile sophisticated selectors during initialization (i.e.
  -- outside the `extract` method): in addition to being more efficient, it
  -- means you'll catch errors during Feedgen startup and not well into runtime.

  local entries = {}

  -- The `:select` method returns an iterator — a callable that returns the next
  -- result for each successive call, or `nil` once results are exhausted. You
  -- can call it directly (for example, if you only need the first matching
  -- element), or use it in the generic `for` loop:
  for tt in html:select("#content > p:first-of-type > tt") do
    -- The Debian news webpage, which I'm using in this example, contains a <p>
    -- element with entries laid out sequentially as follows:
    --
    -- <p>
    --   <tt>[25 Jul 2024]</tt> <strong><a href="./20240725">The Debian Project
    --   mourns the loss of Peter De Schrijver</a></strong><br>
    --   <tt>[29 Jun 2024]</tt> <strong><a href="./20240629">Updated Debian 12:
    --   12.6 released</a></strong><br>
    --   ...
    -- </p>
    --
    -- In other words, an entry consists of:
    -- - the date (the <tt> element)
    -- - the title (the <a> element)
    -- - the link (the `href` attribute of the <a> element)
    -- ...terminated by a <br>. So to extract the entry we have to navigate the
    -- DOM.

    -- The DOM is a tree containing nodes, of which there are several types:
    --
    -- - a document node (contains the top-level nodes: the doctype and the root
    --   element)
    --
    -- - a document fragment node (a separate document root for the contents of
    --   <template> elements)
    --
    -- - a doctype node (<!DOCTYPE html>)
    --
    -- - a comment node (<!-- HTML comments -->)
    --
    -- - a text node (freestanding text: the "hi" in <a>hi</a>)
    --
    -- - an element node (an HTML tag and attributes, like <a href="..."></a>)
    --
    -- - a processing instruction (like <?php ... ?>, exceeding rare in actual
    --   websites)

    -- Each DOM node provides the following methods:
    --
    -- - `node:type`: returns the node type as a string.
    --
    -- - `node:parent`: returns the parent node (or `nil` if there isn't one).
    --
    -- - `node:prevSibling`: returns the preceding element on the same nesting
    --   level (or `nil` if there isn't one).
    -- - `node:nextSibling`: returns the following element on the same nesting
    --   level (or `nil` if there isn't one).
    --
    -- - `node:firstChild`: returns the first child of the node (or `nil` if
    --   the node has no children).
    -- - `node:lastChild`: returns the last child of the node (or `nil` if the
    --   node has no children).
    --
    -- - `node:childNodes`: returns an iterator over the node's immediate
    --   child nodes (first to last).
    -- - `node:descendantNodes`: returns an iterator over the node's
    --   descendants (child nodes, children of the child nodes, etc.); the
    --   traversal is in pre-order.
    --
    -- In addition to these, each node type (except document and fragment)
    -- provides its specific methods.
    --
    -- Doctype:
    -- - `doctype:name`: returns the doctype name ("html" in <!DOCTYPE html>).
    -- - `doctype:publicId`: returns the public ID (used in older HTML
    --   versions), possibly an empty string if not specified.
    -- - `doctype:systemId`: returns the system ID (exceedingly rare),
    --   possibly an empty string if not specified.
    --
    -- Comment and text: provide access to the contents via the `__len` and
    -- `__tostring` metamethods.
    --
    -- Element:
    -- - `element:name`: returns the element name ("a" in <a href="..."></a>).
    --
    -- - `element:html`: converts the element to its HTML string
    --   ("<a href=\"...\"></a>" for <a href="..."></a>).
    -- - `element:innerHtml`: converts the contents of the element to an HTML
    --   string ("inner<br>" for <a href="...">inner<br></a>).
    --
    -- - `element:attr`: returns the value of an attribute (or `nil` if there
    --   isn't one).
    -- - `element:attrs`: returns an iterator over the element's attributes.
    --
    -- - `element:hasClass`: returns `true` if the element has the given CSS
    --   class.
    -- - `element:classes`: returns an iterator over the element's CSS classes.
    --
    -- - `element:text`: returns an iterator over the element's descendant text
    --   nodes' content (the callable returns strings, not node references).
    -- - `element:childElements`: returns an iterator over the element's child
    --   elements (like `element:childNodes`, but skips over non-element
    --   children).
    -- - `element:descendantElements`: returns an iterator over the element's
    --   descendant elements (like `element:descendantNodes`, but skips over
    --   non-element children).
    --
    -- - `element:select`: returns an iterator over the element's descendant
    --   elements that match a CSS selector.
    --
    -- - the `__tostring` metamethod returns a concatenation of the values of
    --   the element's descendant text nodes ("hey there" for <span>hey
    --   <b>there</b></span>).
    --
    -- Processing instruction:
    -- - `pi:target`: the target of the processing instruction ("php" in <?php
    --   ... ?>).
    -- - the `__len` and `__tostring` metamethods provide access to the PI's
    --   data.

    local strong = tt:nextSibling()

    while strong:type() ~= "element" or strong:name() ~= "strong" do
      strong = strong:nextSibling()
    end

    assert(strong:nextSibling():type() == "element")
    assert(strong:nextSibling():name() == "br")

    -- This finds the first matching element.
    local inner = strong:select("a")()

    local day, month, year = tostring(tt):match("(%d+) (%w+) (%d+)")
    local day = tonumber(day)
    local month = months[month]
    local year = tonumber(year)
    local title = inner
    local url = inner:attr("href")

    -- The extractor must return a sequence of entries, each a table with the
    -- following fields:
    table.insert(entries, {
      -- The entry id. Required; must be non-empty.
      -- Note: you can use a string, a number, or a value with a __tostring
      -- metamethod (table/userdata), including DOM elements.
      id = url,

      -- The title. Required; must be non-empty.
      title = title,

      -- The description. Required; may be empty.
      description = title,

      -- The entry's URL. Required; must be valid. Relative URLs are resolved
      -- relative to the source page's URL.
      url = url,

      -- The author. Optional.
      author = "Debian News",

      -- The publication date. Optional.
      pubDate = {
        -- The following group of fields are all required. Must be valid
        -- integers.
        year = year,
        month = month,
        day = day,
        hour = 0,
        minute = 0,
        second = 0,

        -- The date must include timezone information, either via `utcOffset` or
        -- `tz`. If both are provided, the `utcOffset` field is ignored.
        --
        -- The `utcOffset` field must contain an integer offset from UTC in
        -- minutes.
        --
        -- The `tz` field must contain a name of a tz database entry:
        -- https://en.wikipedia.org/wiki/List_of_tz_database_time_zones
        utcOffset = 0,
        tz = "Etc/UTC",

        -- Other fields are ignored.
      },
    })
  end

  return entries
end

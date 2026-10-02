# tRm session helper. Loaded via R_PROFILE_USER; talks to the tRm UI through
# files in $TRM_DIR. Everything is wrapped in tryCatch so a failure here can
# never break the user's R session.
local({
  user_profile <- Sys.getenv("TRM_USER_PROFILE")
  if (nzchar(user_profile) && file.exists(user_profile)) {
    tryCatch(source(user_profile), error = function(e)
      message("tRm: error in ", user_profile, ": ", conditionMessage(e)))
  }
})

local({
  dir <- Sys.getenv("TRM_DIR")
  if (!nzchar(dir)) return(invisible())

  e <- new.env()
  e$dir <- dir
  e$last_plot <- NULL
  # continue numbering after an R restart so old plots stay in the history
  old_plots <- list.files(dir, pattern = "^plot_[0-9]+\\.png$")
  e$plot_n <- if (length(old_plots))
    max(as.integer(gsub("\\D", "", old_plots))) else 0L
  e$view_n <- 0L
  e$max_objs <- 400L

  esc <- function(x) {
    x <- paste(x, collapse = " ")
    x <- gsub("[\t\r\n]", " ", x)
    if (nchar(x, type = "chars", allowNA = TRUE) %in% NA) x <- "?"
    if (nchar(x) > 300) x <- paste0(substr(x, 1, 297), "...")
    x
  }

  write_atomic <- function(lines, name) {
    tmp <- file.path(dir, paste0(name, ".tmp"))
    con <- file(tmp, open = "w", encoding = "UTF-8")
    writeLines(enc2utf8(as.character(lines)), con, useBytes = TRUE)
    close(con)
    file.rename(tmp, file.path(dir, name))
  }

  short_class <- function(x) {
    cl <- class(x)
    if (is.data.frame(x)) {
      if (inherits(x, "tbl_df")) return("tibble")
      if (inherits(x, "data.table")) return("data.table")
      return("data.frame")
    }
    cl[1]
  }

  dims <- function(x) {
    d <- dim(x)
    if (is.data.frame(x)) return(sprintf("%d obs. of %d vars", nrow(x), ncol(x)))
    if (!is.null(d)) return(paste(d, collapse = " x "))
    if (is.function(x) || is.environment(x)) return("")
    paste0("[", length(x), "]")
  }

  value_summary <- function(x) {
    tryCatch({
      if (is.function(x)) {
        a <- names(formals(x))
        return(paste0("function(", paste(a, collapse = ", "), ")"))
      }
      if (is.environment(x)) return(format(x))
      if (is.data.frame(x)) return(dims(x))
      if (is.factor(x)) {
        return(sprintf("Factor w/ %d levels: %s", nlevels(x),
                       paste(head(levels(x), 4), collapse = ", ")))
      }
      if (is.atomic(x) && length(x) > 0) {
        v <- head(x, 8)
        s <- if (is.character(v)) paste0('"', v, '"') else format(v, digits = 4)
        return(paste(s, collapse = " "))
      }
      if (is.list(x)) {
        n <- names(x)
        if (!is.null(n)) return(paste0("List: ", paste(head(n, 6), collapse = ", ")))
        return(sprintf("List of %d", length(x)))
      }
      if (is.null(x)) return("NULL")
      paste(utils::capture.output(utils::str(x, max.level = 0, give.attr = FALSE))[1])
    }, error = function(err) "<?>")
  }

  osize <- function(x) {
    tryCatch(as.numeric(utils::object.size(x)), error = function(err) -1)
  }

  describe <- function(nm, x, out) {
    out[[length(out) + 1L]] <- paste("OBJ", esc(nm), esc(short_class(x)),
                                     format(osize(x), scientific = FALSE),
                                     esc(dims(x)), esc(value_summary(x)), sep = "\t")
    # children: data.frame columns / list elements
    if (is.list(x) && !is.environment(x) && length(x) > 0) {
      n <- names(x)
      k <- min(length(x), 200L)
      for (i in seq_len(k)) {
        cn <- if (!is.null(n) && nzchar(n[i])) n[i] else paste0("[[", i, "]]")
        ch <- x[[i]]
        out[[length(out) + 1L]] <- paste("CHILD", esc(cn), esc(short_class(ch)),
                                         format(osize(ch), scientific = FALSE),
                                         esc(dims(ch)), esc(value_summary(ch)), sep = "\t")
      }
      if (length(x) > k) out[[length(out) + 1L]] <- paste("CHILD", "...", "", "-1", "",
                                                           sprintf("%d more", length(x) - k), sep = "\t")
    }
    # preview text
    prev <- tryCatch({
      w <- getOption("width"); options(width = 200); on.exit(options(width = w))
      if (is.data.frame(x) || is.matrix(x)) {
        utils::capture.output(print(utils::head(x, 15)))
      } else if (is.function(x)) {
        utils::head(deparse(x), 40)
      } else {
        utils::capture.output(utils::str(x, max.level = 1, list.len = 25, vec.len = 4,
                                         give.attr = FALSE))
      }
    }, error = function(err) paste("<preview failed:", conditionMessage(err), ">"))
    for (l in utils::head(prev, 40)) out[[length(out) + 1L]] <- paste("PREV", esc(l), sep = "\t")
    out
  }

  e$write_env <- function() {
    env <- globalenv()
    nms <- sort(ls(env, all.names = FALSE))
    out <- list(paste("TOTAL", length(nms), sep = "\t"))
    for (nm in utils::head(nms, e$max_objs)) {
      out <- tryCatch({
        # never force active bindings or promises silently twice
        x <- get(nm, envir = env, inherits = FALSE)
        describe(nm, x, out)
      }, error = function(err) {
        out[[length(out) + 1L]] <- paste("OBJ", esc(nm), "?", "-1", "", "<error>", sep = "\t")
        out
      })
    }
    write_atomic(unlist(out), "env")
  }

  plot_size <- function() {
    f <- file.path(dir, "plotsize")
    s <- if (file.exists(f)) tryCatch(suppressWarnings(scan(f, quiet = TRUE)),
                                      error = function(err) NULL)
    if (length(s) >= 2 && all(is.finite(s)) && all(s > 50)) return(pmin(s[1:2], 3000))
    c(800, 600)
  }

  # Size of the plot pane in character cells (for the text-mode PDF).
  text_size <- function() {
    f <- file.path(dir, "textsize")
    s <- if (file.exists(f)) tryCatch(suppressWarnings(scan(f, quiet = TRUE)),
                                      error = function(err) NULL)
    if (length(s) >= 2 && all(is.finite(s)) && all(s >= 10)) return(pmin(s[1:2], 1000))
    c(80, 24)
  }

  # Uncompressed vector copy: text stays text, so tRm can draw readable
  # ASCII plots. One cell = 5 x 11 points.
  render_pdf <- function(p, f) {
    ts <- text_size()
    cur <- grDevices::dev.cur()
    opened <- tryCatch({
      grDevices::pdf(f, width = max(ts[1] * 5, 220) / 72, height = max(ts[2] * 11, 160) / 72,
                     compress = FALSE, useDingbats = FALSE)
      grDevices::dev.cur()
    }, error = function(err) NULL)
    if (is.null(opened)) return(invisible(FALSE))
    ok <- tryCatch({ grDevices::replayPlot(p); TRUE }, error = function(err) FALSE)
    tryCatch({
      grDevices::dev.off(opened)
      if (cur > 1) grDevices::dev.set(cur)
    }, error = function(err) NULL)
    if (!ok) unlink(f)
    invisible(ok)
  }

  e$render_plot <- function(p, force = FALSE) {
    sz <- plot_size()
    e$plot_n <- e$plot_n + 1L
    f <- file.path(dir, sprintf("plot_%04d.png", e$plot_n))
    suppressWarnings(render_pdf(p, sub("\\.png$", ".pdf", f)))
    cur <- grDevices::dev.cur()
    opened <- tryCatch({
      if (isTRUE(capabilities("cairo"))) {
        grDevices::png(f, width = sz[1], height = sz[2], type = "cairo", bg = "white")
      } else {
        grDevices::png(f, width = sz[1], height = sz[2], bg = "white")
      }
      grDevices::dev.cur()
    }, error = function(err) NULL)
    if (is.null(opened)) return(invisible(FALSE))
    ok <- tryCatch({
      grDevices::replayPlot(p)
      TRUE
    }, error = function(err) FALSE)
    tryCatch({
      grDevices::dev.off(opened)
      if (cur > 1) grDevices::dev.set(cur)
    }, error = function(err) NULL)
    if (isTRUE(ok)) write_atomic(c(e$plot_n, f), "plot")
    invisible(ok)
  }

  e$check_plot <- function() {
    if (grDevices::dev.cur() <= 1) return(invisible())
    p <- tryCatch(grDevices::recordPlot(), error = function(err) NULL)
    if (is.null(p) || is.null(p[[1]]) || length(p[[1]]) == 0) return(invisible())
    if (!is.null(e$last_plot) && identical(p, e$last_plot)) return(invisible())
    e$last_plot <- p
    e$render_plot(p)
  }

  e$write_state <- function() {
    write_atomic(c(paste("wd", esc(getwd()), sep = "\t"),
                   paste("lib", vapply(.libPaths(), esc, ""), sep = "\t"),
                   paste("pkg", vapply(.packages(), esc, ""), sep = "\t")), "state")
  }

  # Help pages (and anything else shown with file.show) go to the Viewer pane.
  e$pager <- function(files, header = "", title = "R Information", delete.file = FALSE) {
    tryCatch({
      out <- paste("TITLE", esc(title), sep = "\t")
      for (i in seq_along(files)) {
        l <- readLines(files[i], warn = FALSE)
        if (length(header) >= i && nzchar(header[i])) l <- c(header[i], "", l)
        out <- c(out, paste0("L\t", gsub("[\t\r]", "    ", l)))
      }
      write_atomic(out, "help")
    }, error = function(err) message("help: ", conditionMessage(err)))
    if (isTRUE(delete.file)) unlink(files)
    invisible()
  }
  options(pager = e$pager, help_type = "text")

  write_help <- function(title, lines) {
    write_atomic(c(paste("TITLE", esc(title), sep = "\t"),
                   paste0("L\t", gsub("[\t\r]", "    ", lines))), "help")
  }

  # Render the first match of a help() result straight into the Help pane:
  # no "choose a package" menu, other matches are listed at the top.
  e$show_help <- function(h) {
    paths <- as.character(h)
    if (!length(paths)) return(FALSE)
    p <- paths[1]
    pkg <- basename(dirname(dirname(p)))
    w <- if (is.na(e$help_width)) 80L else min(e$help_width, 100L)
    tmp <- tempfile()
    on.exit(unlink(tmp))
    tools::Rd2txt(utils:::.getHelpFile(p), out = tmp,
                  options = list(width = w, underline_titles = FALSE))
    l <- readLines(tmp, warn = FALSE)
    others <- setdiff(basename(dirname(dirname(paths))), pkg)
    if (length(others))
      l <- c(paste0("Also documented in: ", paste(others, collapse = ", "),
                    "  (search for pkg::topic in the Help pane)"), "", l)
    write_help(paste0(pkg, "::", attr(h, "topic")), l)
    TRUE
  }

  e$help_width <- NA_integer_
  e$sync_help_width <- function() {
    f <- file.path(dir, "viewwidth")
    if (!file.exists(f)) return(invisible())
    w <- suppressWarnings(as.integer(readLines(f, warn = FALSE)[1]))
    if (!is.na(w) && w >= 30 && !identical(w, e$help_width)) {
      e$help_width <- w
      tools::Rd2txt_options(width = min(w, 100L))
    }
    invisible()
  }

  e$callback <- function(expr, value, ok, visible) {
    tryCatch(e$sync_help_width(), error = function(err) NULL)
    tryCatch(e$check_plot(), error = function(err) NULL)
    tryCatch(e$write_env(), error = function(err) NULL)
    tryCatch(e$write_state(), error = function(err) NULL)
    TRUE
  }

  # Off-screen recording device: plots are recorded and replayed into PNGs.
  trm_device <- function(...) {
    grDevices::pdf(NULL, width = 8, height = 6)
    grDevices::dev.control("enable")
  }
  options(device = trm_device)

  tools <- new.env()
  tools$.trm <- e
  tools$View <- function(x, title) {
    if (missing(title)) title <- paste(deparse(substitute(x))[1])
    tryCatch({
      d <- as.data.frame(x, stringsAsFactors = FALSE)
      n <- nrow(d)
      d <- utils::head(d, 2000)
      cols <- vapply(d, function(col) {
        if (is.list(col)) "list" else class(col)[1]
      }, "")
      cells <- lapply(d, function(col) {
        if (is.list(col)) vapply(col, function(v) esc(value_summary(v)), "")
        else if (is.numeric(col)) vapply(as.character(signif(col, 7)), esc, "", USE.NAMES = FALSE)
        else vapply(as.character(col), esc, "", USE.NAMES = FALSE)
      })
      rows <- if (length(cells)) do.call(paste, c(cells, sep = "\t")) else character(0)
      rn <- vapply(rownames(d), esc, "", USE.NAMES = FALSE)
      lines <- c(paste("TITLE", esc(title), n, ncol(d), sep = "\t"),
                 paste(c("HEAD", "", vapply(names(d), esc, "")), collapse = "\t"),
                 paste(c("TYPE", "", cols), collapse = "\t"),
                 if (length(rows)) paste("ROW", rn, rows, sep = "\t"))
      e$view_n <- e$view_n + 1L
      write_atomic(lines, "view")
    }, error = function(err) message("View(): ", conditionMessage(err)))
    invisible(x)
  }
  tools$`?` <- function(e1, e2) {
    mc <- match.call()
    mc[[1L]] <- quote(utils::`?`)
    h <- eval(mc, parent.frame())
    if (inherits(h, "help_files_with_topic") &&
        isTRUE(tryCatch(e$show_help(h), error = function(err) FALSE)))
      return(invisible())
    h
  }
  tools$help <- function(topic, package = NULL, lib.loc = NULL,
                         verbose = getOption("verbose"),
                         try.all.packages = getOption("help.try.all.packages"),
                         help_type = getOption("help_type")) {
    mc <- match.call()
    mc[[1L]] <- quote(utils::help)
    h <- eval(mc, parent.frame())
    if (inherits(h, "help_files_with_topic") &&
        isTRUE(tryCatch(e$show_help(h), error = function(err) FALSE)))
      return(invisible())
    h
  }
  tools$.trm_redraw <- function() {
    if (!is.null(e$last_plot)) e$render_plot(e$last_plot)
    invisible()
  }
  # The profile runs before the default packages are attached; attach our
  # tools after utils so our View() wins over utils::View().
  do_attach <- function(...) {
    if ("tools:trm" %in% search()) detach("tools:trm", character.only = TRUE)
    attach(tools, name = "tools:trm", warn.conflicts = FALSE)
  }
  if ("package:utils" %in% search()) do_attach() else
    setHook(packageEvent("utils", "attach"), do_attach)

  options(setWidthOnResize = TRUE)
  w <- suppressWarnings(as.integer(Sys.getenv("TRM_COLS")))
  if (!is.na(w) && w >= 20) options(width = w)
  addTaskCallback(e$callback, name = "trm")
  e$write_state()
  e$write_env()
  tryCatch(e$sync_help_width(), error = function(err) NULL)
  invisible(NULL)
})

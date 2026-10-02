# Example tRm script: a small tidyverse pipeline with base R pipes (|>)
#
# Put the cursor on a line and press Ctrl+Enter to run it (or the whole
# multi-line pipeline it belongs to). Ctrl+A then Ctrl+Enter runs everything.
# Watch the Environment pane fill up and the plot appear in the Plots pane.
# Toggle the outline with Ctrl+Shift+O (or Alt+O).

# Setup ----
library(dplyr)
library(tidyr)
library(ggplot2)

# Data: fuel economy of 234 cars (ships with ggplot2)
cars <- mpg
glimpse(cars)

# Clean ----
cars_clean <- cars |>
  mutate(
    class = factor(class),
    drive = recode(drv, f = "front", r = "rear", `4` = "4wd"),
    kpl   = hwy * 0.425   # highway miles per gallon -> km per litre
  ) |>
  filter(year == 2008)

# Summarise ----
by_class <- cars_clean |>
  group_by(class) |>
  summarise(
    n        = n(),
    mean_hwy = mean(hwy),
    mean_cty = mean(cty),
    .groups  = "drop"
  ) |>
  arrange(desc(mean_hwy))

by_class

# Reshape to long format for plotting city vs. highway side by side
by_class_long <- by_class |>
  pivot_longer(c(mean_hwy, mean_cty), names_to = "road", values_to = "mpg") |>
  mutate(road = recode(road, mean_hwy = "highway", mean_cty = "city"))

# Helper ----
# A small function using the native lambda syntax \(x)
top_n_models <- function(data, n = 3) {
  data |>
    slice_max(hwy, n = n, with_ties = FALSE) |>
    select(manufacturer, model, hwy) |>
    (\(d) { cat("Top", n, "models by highway mpg:\n"); d })()
}

top_n_models(cars_clean, n = 5)

# Plot ----
ggplot(cars_clean, aes(displ, hwy, colour = drive)) +
  geom_point(size = 2, alpha = 0.7) +
  geom_smooth(method = "lm", se = FALSE, formula = y ~ x) +
  labs(
    title  = "Bigger engines, fewer miles per gallon",
    x      = "Engine displacement (litres)",
    y      = "Highway miles per gallon",
    colour = "Drive"
  ) +
  theme_minimal()

# Second plot: use Left/Right in the Plots pane to flip between plots
by_class_long |>
  ggplot(aes(reorder(class, mpg), mpg, fill = road)) +
  geom_col(position = "dodge") +
  coord_flip() +
  labs(x = NULL, y = "Mean miles per gallon", fill = NULL) +
  theme_minimal()

# Try next ----
# View(by_class)       opens the table in the Viewer pane
# ?pivot_longer        opens help in the Help pane

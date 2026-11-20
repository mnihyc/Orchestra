# Fonts

Inter and JetBrains Mono are part of the build: the bundler takes their files from the packages that
ship them under the SIL Open Font License and writes them under `/fonts/`, named by their content.
Normal Latin text fonts are preloaded; every face stays an external file under `font-src 'self'`.
Nothing is fetched from another site, and no font file is kept here by hand.

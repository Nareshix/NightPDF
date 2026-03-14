wip
pdf-viewer features
- [x] select texts
- [x] search (ctrl-f)
- [x] open pdf file (ctrl-o)
- [x] ctrl a to select all texts
- [x] ctrl c to copy selected texts
- [] ctrl s to save pdf file
- [] ctrl p to print pdf file
- [] highlighting texts (not select)
- [] add images to pdf file
- [] add comments to pdf file
- [] add texts to pdf file (text box)
- [] draw on pdf file
- [x] support for dark mode (machhiato?)
- [] pgdown and pgup to scroll pages
- [] rotate pages
- [x] goto page number (ctrl g)
- [] signature
- [] jump to links in pdf
- [x] zoom (ctrl +/-) pinch is too difficult :/ next time maybe
- [] hover links to show tooltip?
- [] direct editing of pdf files?
- [x] save last seen page and open pdf file at that page
- [] support for opening multiple pdf files in tabs?
- [] have option to not invert images in dark mode (or specific images and remembers them.)
- [] wasm
- [] accelerated scrolling
- [] toggle diff colours quicklky (mainly dark and light mode). or have an option to NOT invert colours for detected images or selected.

Drag & drop to open — drop a PDF onto the window instead of using the file dialog
Recent files list — remember last N opened files, show in a menu
Keyboard scrolling — Page Up/Down, arrow keys, Home/End to jump to start/end
Fit to width / fit to page zoom presets instead of only +/- increments
Window title shows filename — currently probably just "PDF Dark Reader" always
Rotate page — some PDFs open sideways


Table of contents / outline panel — pdfium exposes this, side panel with clickable headings
Page thumbnails sidebar — visual strip of pages on the left for quick navigation
Invert colors toggle — useful for dark theme, separate from the theme system
Ctrl+scroll to zoom — standard in most document viewers
Case-sensitive search toggle — currently always case-insensitive presumably

ocr?

use egui-phosphor for icons rn folder icon look weird on linux
ctrl + scroll not implemented. might do in future if i get a mouse


# Todo
rfd needs to be spawned in another thread
search hangs main thread if pdf is large. need to spawn in another thread

need to log any errors
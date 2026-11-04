PubConv - Publisher (.pub) to PDF converter
============================================

PubConv converts Microsoft Publisher files to PDF on your own computer.
Nothing is uploaded and no internet connection is needed. It keeps what
other converters lose:

  * WordArt headlines (curved, slanted, arched, wavy, vertical)
  * Newsletters whose text flows between linked text boxes and pages

Everything PubConv needs is included. You don't need Microsoft Publisher,
LibreOffice or anything else.

How to use
----------
1. Open pubconv.exe.
2. Click "Add files..." or "Add folder...", or drag files and folders onto
   the window.
3. Choose where to save the PDFs (next to each file, or one folder).
4. Click "Convert to PDF". Double-click a finished file to open its PDF.

Your original .pub files are never changed.

For IT staff: command line
--------------------------
  pubconv-cli.exe "D:\Archive" -r -o "D:\PDF"

  -o, --out DIR     save PDFs in DIR instead of next to each file
  -r, --recurse     include subfolders
  --keep-temp       keep working files (troubleshooting)

Exit code 0 = all converted, 1 = some failed, 2 = usage error.

Good to know
------------
  * Install the fonts your documents use for the closest match. Missing
    fonts fall back to similar ones.
  * Rainbow / multi-colour WordArt gradients and 3D effects are drawn in a
    single colour.

Licence
-------
PubConv's own code: Mozilla Public License 2.0 (LICENSE-MPL-2.0.txt).
The runtime folder contains LibreOffice components (MPL 2.0); their licence
and notices are in runtime\. Other components: THIRD-PARTY-NOTICES.txt.
Microsoft and Publisher are trademarks of Microsoft Corporation. PubConv is
not affiliated with Microsoft.

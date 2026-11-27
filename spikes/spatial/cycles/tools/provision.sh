#!/bin/bash
# Build the pinned Cycles bundle that nothing-spatial-cycles links, into .local/cycles at the
# repository root. Needs Xcode, git, git-lfs, ninja, CMake and python3. Downloads about 2.6 GB.
#
#   spikes/spatial/cycles/tools/provision.sh
#
# The result is .local/cycles/install: libn4cycles.dylib, lib/ and the kernel source Cycles
# compiles for Metal at run time. Nothing under .local is tracked.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="$(cd "$HERE/../../.." && pwd)"
W="$ROOT/.local/cycles"
mkdir -p "$W" && cd "$W"

if [ ! -d cycles ]; then
  git clone --depth 1 --branch v5.3.0 https://projects.blender.org/blender/cycles.git cycles
  (cd cycles && python3 src/cmake/make_update.py && git apply "$HERE/patches/0001-add-n4-bridge-subdirectory.patch")
fi
cmake -S cycles -B cycles-build -G Ninja -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$W/install" -DN4_BRIDGE_DIR="$HERE/native" \
  -DWITH_CYCLES_USD=OFF -DWITH_CYCLES_HYDRA_RENDER_DELEGATE=OFF -DWITH_CYCLES_ALEMBIC=OFF \
  -DWITH_CYCLES_OSL=OFF -DWITH_CYCLES_OPENVDB=OFF -DWITH_CYCLES_NANOVDB=OFF -DWITH_CYCLES_OPENSUBDIV=OFF
# Re-installing the standalone `cycles` tool can report harmless install_name_tool errors.
cmake --build cycles-build --target install || test -f install/libn4cycles.dylib

# Cycles' precompiled OpenImageIO statically includes libheif (LGPL-3). Replace it with a build
# that has no HEIF support and depends only on libraries Cycles already ships.
L="$W/cycles/lib/macos_arm64"
[ -d oiio ] || git clone --depth 1 --branch v3.1.16.0 https://github.com/AcademySoftwareFoundation/OpenImageIO.git oiio
[ -d tiff ] || git clone --depth 1 --branch v4.7.1 https://gitlab.com/libtiff/libtiff.git tiff
cmake -S tiff -B tiff-build -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$W/tiff-install" \
  -DCMAKE_OSX_DEPLOYMENT_TARGET=13.0 -DBUILD_SHARED_LIBS=OFF -DCMAKE_POSITION_INDEPENDENT_CODE=ON -DCMAKE_INSTALL_LIBDIR=lib \
  -DCMAKE_FIND_FRAMEWORK=NEVER -DCMAKE_IGNORE_PREFIX_PATH="/opt/homebrew;/usr/local" \
  -Dtiff-tools=OFF -Dtiff-contrib=OFF -Dtiff-tests=OFF -Dtiff-docs=OFF -Dwebp=OFF -Dzstd=OFF -Dlzma=OFF -Djbig=OFF \
  -Dlerc=OFF -Dlibdeflate=OFF -Djpeg=OFF -Dold-jpeg=OFF -Djpeg12=OFF -Dpixarlog=ON -Dzlib=ON -Dcxx=OFF
cmake --build tiff-build --target install
cmake -S oiio -B oiio-build -G Ninja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$W/oiio-install" \
  -DCMAKE_OSX_DEPLOYMENT_TARGET=13.0 -DCMAKE_CXX_STANDARD=20 -DCMAKE_FIND_FRAMEWORK=NEVER \
  -DCMAKE_IGNORE_PREFIX_PATH="/opt/homebrew;/usr/local" \
  -DCMAKE_PREFIX_PATH="$L/imath;$L/openexr;$L/opencolorio;$L/tbb;$L/fmt;$L/jpeg;$L/png;$L/zstd;$L/openjph" \
  -DJPEG_LIBRARY="$L/jpeg/lib/libjpeg.a" -DJPEG_INCLUDE_DIR="$L/jpeg/include" -DTIFF_ROOT="$W/tiff-install" \
  -DBUILD_SHARED_LIBS=ON -DOIIO_BUILD_TOOLS=OFF -DOIIO_BUILD_TESTS=OFF -DBUILD_TESTING=OFF -DUSE_PYTHON=OFF -DUSE_QT=OFF \
  -DBUILD_DOCS=OFF -DINSTALL_DOCS=OFF -DSTOP_ON_WARNING=OFF \
  -DCMAKE_DISABLE_FIND_PACKAGE_Libheif=ON -DCMAKE_DISABLE_FIND_PACKAGE_FFmpeg=ON -DCMAKE_DISABLE_FIND_PACKAGE_LibRaw=ON \
  -DCMAKE_DISABLE_FIND_PACKAGE_OpenCV=ON -DCMAKE_DISABLE_FIND_PACKAGE_GIF=ON -DCMAKE_DISABLE_FIND_PACKAGE_WebP=ON \
  -DCMAKE_DISABLE_FIND_PACKAGE_OpenJPEG=ON -DCMAKE_DISABLE_FIND_PACKAGE_JXL=ON -DCMAKE_DISABLE_FIND_PACKAGE_Ptex=ON \
  -DCMAKE_DISABLE_FIND_PACKAGE_OpenVDB=ON -DCMAKE_DISABLE_FIND_PACKAGE_DCMTK=ON -DCMAKE_DISABLE_FIND_PACKAGE_Freetype=ON \
  -DCMAKE_DISABLE_FIND_PACKAGE_BZip2=ON -DCMAKE_DISABLE_FIND_PACKAGE_libuhdr=ON
cmake --build oiio-build --target install
I=install/lib
cp oiio-install/lib/libOpenImageIO.3.1.16.dylib $I/libOpenImageIO.dylib
cp oiio-install/lib/libOpenImageIO_Util.3.1.16.dylib $I/libOpenImageIO_Util.dylib
chmod u+w $I/libOpenImageIO.dylib $I/libOpenImageIO_Util.dylib
install_name_tool -id @rpath/libOpenImageIO.dylib -change @rpath/libOpenImageIO_Util.3.1.dylib @rpath/libOpenImageIO_Util.dylib $I/libOpenImageIO.dylib
install_name_tool -id @rpath/libOpenImageIO_Util.dylib $I/libOpenImageIO_Util.dylib
codesign -f -s - $I/libOpenImageIO.dylib $I/libOpenImageIO_Util.dylib
cp tiff/LICENSE.md install/license/libtiff-LICENSE.md
cp oiio/LICENSE.md install/license/OpenImageIO-LICENSE.md
if nm $I/libOpenImageIO.dylib | grep -qi heif_; then echo "HEIF symbols still present" >&2; exit 1; fi
echo "Cycles bundle ready at $W/install"

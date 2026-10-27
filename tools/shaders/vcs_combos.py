"""Developer tool: a CS2 shader's static combos (VCS format 72), decompiled to GLSL, to read Valve's shader maths
(M10, the paint styles: docs/research/cs2-skins.md, "Reading CS2's paint shader"). What it writes is Valve's
content: keep it out of the repository.

The steps, with Source 2 Viewer's command-line build (https://github.com/ValveResourceFormat/ValveResourceFormat,
MIT; 20.1 or newer reads format 72), which also ships SPIRV-Cross as spirv-cross.dll:

  Source2Viewer-CLI -i <csgo>/shaders_vulkan_dir.vpk -f shaders/vfx/csgo_customweapon_vulkan_50_ps.vcs -o out/
  Source2Viewer-CLI -i out/shaders/vfx/csgo_customweapon_vulkan_50_ps.vcs -b DATA > dump.txt
  python tools/shaders/vcs_combos.py table dump.txt 0 288      -> combo, block offset, block size, bytecode offset
  cargo run --release --example shader_blocks -- shaders/vfx/csgo_customweapon_vulkan_50_ps.vcs \
      vfx_zstd_255df362.bin blocks/ <block offset>...
  python tools/shaders/vcs_combos.py glsl dump.txt <combo> blocks/block_<offset>.bin <bytecode offset> combo.glsl

The zstd dictionary is in Source 2 Viewer's repository (ValveResourceFormat/Utils/vfx_zstd_255df362.bin). The GLSL
gets its names as Source 2 Viewer gives them: the globals' members by offset / 4, textures by binding - 30 (format
69 and newer), the two samplers by their bindings (22 wrap, 23 clamp). Set OFAST_SPIRV_CROSS to spirv-cross.dll.

  vcs_combos.py table <dump.txt> <combo id>...
  vcs_combos.py glsl <dump.txt> <combo id> <block.bin> <bytecode offset> <out.glsl>
"""
import ctypes
import os
import re
import struct
import sys


class Dump:
    def __init__(self, path):
        self.text = open(path, encoding="utf-8", errors="replace").read()
        self.ids = [int(v) for v in re.findall(r"-?\d+", self.section("m_staticComboIDs").split("=", 1)[1])]
        combo_data = self.section("m_staticComboData")
        self.entries = re.split(r"\n\t\t\{\n", combo_data)[1:]
        self.code_idx = [int(re.search(r"m_nByteCodeDataIdx = (-?\d+)", e).group(1)) for e in self.entries]
        self.byte_index = [[int(x) for x in re.search(r"m_byteCodeIndex = \[([^\]]*)\]", e).group(1).split(",") if x.strip()]
                           for e in self.entries]
        code = self.section("m_byteCodeData")
        self.offs = [int(v) for v in re.findall(r"m_nOffs = (\d+)", code)]
        self.sizes = [int(v) for v in re.findall(r"m_nSize = (\d+)", code)]
        self.inner = [[int(x) for x in re.findall(r"\d+", v)] for v in re.findall(r"m_offs = \[([^\]]*)\]", code)]
        variables = re.split(r"\n\t\t\{\n", self.section("m_variableDescriptionArray"))[1:]
        self.names = [re.search(r'm_szName = "([^"]*)"', v).group(1) for v in variables]
        assert len(self.ids) == len(self.entries), (len(self.ids), len(self.entries))

    def section(self, name):
        start = self.text.index(f"\t{name} = ")
        nxt = re.search(r"\n\tm_\w+ = ", self.text[start + 10:])
        return self.text[start:start + 10 + nxt.start()] if nxt else self.text[start:]

    def fields(self, combo):
        """(render state fields, constant fields) of a combo's m_allVars: (variable index, layout set, dest/field2)."""
        entry = self.entries[self.ids.index(combo)]
        all_vars = entry[entry.index("m_allVars"):]
        values = [int(v) for v in re.search(r"m_indexAndRegisterOffsetArray = \s*\[([^\]]*)\]", all_vars).group(1).replace("\n", " ").split(",") if v.strip()]
        first_rs = int(re.search(r"m_nFirstRenderStateElement = (\d+)", all_vars).group(1))
        first_c = int(re.search(r"m_nFirstConstantElement = (\d+)", all_vars).group(1))
        unpacked = [((v >> 16) & 0xFFF, (v >> 16) >> 12, v & 0xFFFF) for v in values]
        return unpacked[first_rs:first_c], unpacked[first_c:]

    def row(self, combo):
        i = self.ids.index(combo)
        k = self.code_idx[i]
        return self.offs[k], self.sizes[k], self.inner[k][self.byte_index[i][0]]


class Resource(ctypes.Structure):
    _fields_ = [("id", ctypes.c_uint32), ("base_type_id", ctypes.c_uint32), ("type_id", ctypes.c_uint32),
                ("name", ctypes.c_char_p)]


def glsl(dump, combo, block_path, offset, out_path):
    data = open(block_path, "rb").read()
    version, size = struct.unpack_from("<ii", data, offset)
    spirv = data[offset + 8:offset + 8 + size]
    assert struct.unpack_from("<I", spirv, 0)[0] == 0x07230203, "not SPIR-V"
    lib = ctypes.CDLL(os.environ["OFAST_SPIRV_CROSS"])
    ctx, ir, comp, opts = ctypes.c_void_p(), ctypes.c_void_p(), ctypes.c_void_p(), ctypes.c_void_p()
    words = (ctypes.c_uint32 * (size // 4)).from_buffer_copy(spirv)
    lib.spvc_context_get_last_error_string.restype = ctypes.c_char_p
    lib.spvc_compiler_get_type_handle.restype = ctypes.c_void_p
    lib.spvc_type_get_num_member_types.restype = ctypes.c_uint
    lib.spvc_compiler_get_decoration.restype = ctypes.c_uint

    def check(result, what):
        if result != 0:
            sys.exit(f"{what}: {result} {lib.spvc_context_get_last_error_string(ctx)}")

    check(lib.spvc_context_create(ctypes.byref(ctx)), "context")
    check(lib.spvc_context_parse_spirv(ctx, words, ctypes.c_size_t(size // 4), ctypes.byref(ir)), "parse")
    # SPVC_BACKEND_GLSL = 1, SPVC_CAPTURE_MODE_TAKE_OWNERSHIP = 1
    check(lib.spvc_context_create_compiler(ctx, 1, ir, 1, ctypes.byref(comp)), "compiler")
    check(lib.spvc_compiler_create_compiler_options(comp, ctypes.byref(opts)), "options")
    glsl_bit = 0x2000000  # spirv_cross_c.h: GLSL_VERSION 8, ES 9, VULKAN_SEMANTICS 10, EMIT_UNIFORM_BUFFER_AS_PLAIN_UNIFORMS 35
    check(lib.spvc_compiler_options_set_uint(opts, ctypes.c_int(glsl_bit | 8), ctypes.c_uint(460)), "version")
    check(lib.spvc_compiler_options_set_bool(opts, ctypes.c_int(glsl_bit | 9), ctypes.c_ubyte(0)), "es")
    check(lib.spvc_compiler_options_set_bool(opts, ctypes.c_int(glsl_bit | 10), ctypes.c_ubyte(1)), "vulkan")
    check(lib.spvc_compiler_options_set_bool(opts, ctypes.c_int(glsl_bit | 35), ctypes.c_ubyte(1)), "plain uniforms")
    check(lib.spvc_compiler_install_compiler_options(comp, opts), "install")

    render_state, constants = dump.fields(combo)
    resources = ctypes.c_void_p()
    check(lib.spvc_compiler_create_shader_resources(comp, ctypes.byref(resources)), "resources")

    def resource_list(kind):
        items, count = ctypes.POINTER(Resource)(), ctypes.c_size_t()
        check(lib.spvc_resources_get_resource_list_for_type(resources, kind, ctypes.byref(items), ctypes.byref(count)), "list")
        return [items[i] for i in range(count.value)]

    def decoration(res_id, which):  # SpvDecorationDescriptorSet 34, Binding 33
        return lib.spvc_compiler_get_decoration(comp, ctypes.c_uint(res_id), ctypes.c_int(which))

    # Uniform buffers (1): the globals' members by offset / 4 == field2.
    for res in resource_list(1):
        lib.spvc_compiler_set_name(comp, ctypes.c_uint(res.id), b"_Globals_")
        handle = lib.spvc_compiler_get_type_handle(comp, ctypes.c_uint(res.base_type_id))
        for m in range(lib.spvc_type_get_num_member_types(ctypes.c_void_p(handle))):
            off = ctypes.c_uint()
            check(lib.spvc_compiler_type_struct_member_offset(comp, ctypes.c_void_p(handle), ctypes.c_uint(m), ctypes.byref(off)), "offset")
            name = next((dump.names[v] for v, _, f2 in constants if f2 == off.value // 4), None)
            if name:
                lib.spvc_compiler_set_member_name(comp, ctypes.c_uint(res.base_type_id), ctypes.c_uint(m), name.encode())
    # Separate images (10) and samplers (11): render-state fields with dest == binding - 30 / - 14 in their set.
    for res in resource_list(11):
        binding = decoration(res.id, 33)
        name = {22: "g_sTrilinearWrap", 23: "g_sTrilinearClamp"}.get(binding, f"g_sSampler{binding}")
        lib.spvc_compiler_set_name(comp, ctypes.c_uint(res.id), name.encode())
    for kind, start, prefix in [(10, 30, "g_t")]:
        for res in resource_list(kind):
            binding, layout = decoration(res.id, 33), decoration(res.id, 34)
            candidates = [dump.names[v] for v, s, f2 in render_state if s == layout and (f2 & 0xFF) == binding - start]
            name = next((n for n in candidates if n.startswith(prefix)), candidates[0] if candidates else None)
            if name:
                lib.spvc_compiler_set_name(comp, ctypes.c_uint(res.id), name.encode())

    source = ctypes.c_char_p()
    check(lib.spvc_compiler_compile(comp, ctypes.byref(source)), "compile")
    open(out_path, "w", encoding="utf-8", newline="\n").write(source.value.decode())
    print(f"{out_path}: combo {combo}, version {version}, {size} bytes of SPIR-V")


if __name__ == "__main__":
    dump = Dump(sys.argv[2])
    if sys.argv[1] == "table":
        for combo in [int(v) for v in sys.argv[3:]]:
            print(combo, *dump.row(combo))
    else:
        glsl(dump, int(sys.argv[3]), sys.argv[4], int(sys.argv[5]), sys.argv[6])

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn run(command: &mut Command) -> Output {
    let output = command.output().expect("start command");
    assert!(
        output.status.success(),
        "command failed: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn shared_library(directory: &Path) -> PathBuf {
    directory.join(if cfg!(target_os = "macos") {
        "libsdk_fixture.dylib"
    } else {
        "libsdk_fixture.so"
    })
}

#[test]
fn generated_host_sdks_call_a_real_library() {
    let directory = std::env::temp_dir().join(format!("lulang-sdk-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create SDK fixture directory");
    let source = directory.join("sdk_fixture.lu");
    std::fs::write(
        &source,
        "@c_layout type Vec2 { x: f64, y: f64 }\n\
         @c_layout type LuResultI64 { status: i64, value: i64 }\n\
         export fn borrowed_sum(values: c_slice[f64]): f64 {\n\
           var total = 0.0\n\
           for i in 0..len(values) { total = total + values[i] }\n\
           return total\n\
         }\n\
         export fn borrowed_bump(values: c_mut_slice[f64]): f64 {\n\
           for i in 0..len(values) { values[i] = values[i] + 1.0 }\n\
           return values[0]\n\
         }\n\
         export fn vec2_sum(value: Vec2): f64 { return value.x + value.y }\n\
         export fn make_vec2(x: f64, y: f64): Vec2 { return Vec2 { x, y } }\n\
         export fn make_values(count: i64): [f64] {\n\
           var values = arr(count, 0.0)\n\
           for i in 0..count { values[i] = float(i) * 0.5 }\n\
           return values\n\
         }\n\
         export fn positive(value: i64): bool { return value > 0 }\n\
         export fn checked_div(numerator: i64, denominator: i64): LuResultI64 {\n\
           if denominator == 0 { return LuResultI64 { 1, 0 } }\n\
           return LuResultI64 { 0, numerator / denominator }\n\
         }\n\
         export fn callback_identity(\n\
           callback: c_fn[(i64) -> i64],\n\
         ): c_fn[(i64) -> i64] { return callback }\n\
         export fn greeting(prefix: str): str { return concat(prefix, \"\\0!\") }\n\
         main { print(0) }\n",
    )
    .expect("write SDK fixture");
    let base = directory.join("sdk_fixture");

    run(Command::new(env!("CARGO_BIN_EXE_lu"))
        .args(["build", "--lib", "-o"])
        .arg(&base)
        .arg(&source));
    let manifest = directory.join("sdk_fixture.json");
    let rust_sdk = directory.join("sdk_fixture.rs");
    let cpp_sdk = directory.join("sdk_fixture.hpp");
    let julia_sdk = directory.join("sdk_fixture.jl");
    let node_sdk = directory.join("sdk_fixture-node");
    let go_sdk = directory.join("sdk_fixture-go");
    let swift_sdk = directory.join("sdk_fixture-swift");
    let r_sdk = directory.join("sdk_fixture-r");
    for (language, output) in [
        ("rust", &rust_sdk),
        ("cpp", &cpp_sdk),
        ("julia", &julia_sdk),
    ] {
        run(Command::new(env!("CARGO_BIN_EXE_lu"))
            .args(["sdk", language, "-o"])
            .arg(output)
            .arg(&manifest));
    }
    for (language, output) in [
        ("node", &node_sdk),
        ("go", &go_sdk),
        ("swift", &swift_sdk),
        ("r", &r_sdk),
    ] {
        run(Command::new(env!("CARGO_BIN_EXE_lu"))
            .args(["sdk", language, "-o"])
            .arg(output)
            .arg(&manifest));
    }

    let rust = std::fs::read_to_string(&rust_sdk).expect("read Rust SDK");
    assert!(rust.contains("values: &mut [f64]"));
    assert!(rust.contains("values.as_mut_ptr()"));
    assert!(rust.contains("values: &[f64]"));
    let rust_harness = directory.join("rust_harness.rs");
    std::fs::write(
        &rust_harness,
        format!(
            "{rust}\n\
             fn main() {{\n\
               unsafe extern \"C\" fn increment(value: i64) -> i64 {{ value + 1 }}\n\
               let values = [1.0, 2.0, 3.0];\n\
               assert_eq!(borrowed_sum(&values), 6.0);\n\
               let mut values = [1.5, 2.5];\n\
               assert_eq!(borrowed_bump(&mut values), 2.5);\n\
               assert_eq!(values, [2.5, 3.5]);\n\
               assert_eq!(vec2_sum(Vec2 {{ x: 2.0, y: 5.0 }}), 7.0);\n\
               let made = make_vec2(1.25, 3.75);\n\
               assert_eq!((made.x, made.y), (1.25, 3.75));\n\
               let mut owned = make_values(4);\n\
               assert_eq!(owned.as_slice(), &[0.0, 0.5, 1.0, 1.5]);\n\
               owned.as_mut_slice()[0] = 9.0;\n\
               assert_eq!(owned.as_slice()[0], 9.0);\n\
               assert!(positive(3));\n\
               assert_eq!(checked_div(12, 3), Ok(4));\n\
               assert_eq!(checked_div(12, 0), Err(LuError {{ code: 1 }}));\n\
               let callback = callback_identity(Some(increment)).unwrap();\n\
               assert_eq!(unsafe {{ callback(41) }}, 42);\n\
               assert_eq!(greeting(b\"A\"), vec![65, 0, 33]);\n\
               println!(\"rust sdk ok\");\n\
             }}\n"
        ),
    )
    .expect("write Rust harness");
    let rust_binary = directory.join("rust_harness");
    run(Command::new("rustc")
        .arg("--edition=2021")
        .arg(&rust_harness)
        .arg("-L")
        .arg(format!("native={}", directory.display()))
        .arg("-o")
        .arg(&rust_binary));
    let output = run(&mut Command::new(&rust_binary));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "rust sdk ok\n");

    let cpp = std::fs::read_to_string(&cpp_sdk).expect("read C++ SDK");
    assert!(cpp.contains("std::span<double> values"));
    assert!(cpp.contains("std::span<const double> values"));
    let cpp_harness = directory.join("cpp_harness.cpp");
    std::fs::write(
        &cpp_harness,
        "#include \"sdk_fixture.hpp\"\n\
         #include <cassert>\n\
         #include <iostream>\n\
         #include <vector>\n\
         static int64_t increment(int64_t value) { return value + 1; }\n\
         int main() {\n\
           const std::vector<double> input{1.0, 2.0, 3.0};\n\
           assert(sdk_fixture::borrowed_sum(std::span<const double>(input)) == 6.0);\n\
           std::vector<double> values{1.5, 2.5};\n\
           assert(sdk_fixture::borrowed_bump(std::span<double>(values)) == 2.5);\n\
           assert(values[0] == 2.5 && values[1] == 3.5);\n\
           assert(sdk_fixture::vec2_sum(Vec2{2.0, 5.0}) == 7.0);\n\
           const Vec2 made = sdk_fixture::make_vec2(1.25, 3.75);\n\
           assert(made.x == 1.25 && made.y == 3.75);\n\
           auto owned = sdk_fixture::make_values(4);\n\
           assert(owned.span().size() == 4 && owned.span()[3] == 1.5);\n\
           owned.span()[0] = 9.0;\n\
           assert(owned.span()[0] == 9.0);\n\
           assert(sdk_fixture::positive(3));\n\
           const auto divided = sdk_fixture::checked_div(12, 3);\n\
           const auto failed = sdk_fixture::checked_div(12, 0);\n\
           assert(divided && divided.value == 4);\n\
           assert(!failed && failed.error.code == 1);\n\
           const auto callback = sdk_fixture::callback_identity(increment);\n\
           assert(callback(41) == 42);\n\
           const std::string greeting = sdk_fixture::greeting(\"A\");\n\
           assert(greeting.size() == 3 && greeting[0] == 'A' && greeting[1] == 0 && greeting[2] == '!');\n\
           std::cout << \"cpp sdk ok\\n\";\n\
         }\n",
    )
    .expect("write C++ harness");
    let cpp_binary = directory.join("cpp_harness");
    run(Command::new("clang++")
        .arg("-std=c++20")
        .arg("-O2")
        .arg("-I")
        .arg(&directory)
        .arg(&cpp_harness)
        .arg(directory.join("libsdk_fixture.a"))
        .arg("-o")
        .arg(&cpp_binary));
    let output = run(&mut Command::new(&cpp_binary));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "cpp sdk ok\n");

    let julia = std::fs::read_to_string(&julia_sdk).expect("read Julia SDK");
    assert!(julia.contains("values::Vector{Float64}"));
    assert!(julia.contains("GC.@preserve values begin"));
    if Command::new("julia").arg("--version").output().is_ok() {
        run(Command::new(env!("CARGO_BIN_EXE_lu"))
            .args(["build", "--lib", "--shared", "-o"])
            .arg(&base)
            .arg(&source));
        let julia_harness = directory.join("julia_harness.jl");
        std::fs::write(
            &julia_harness,
            "include(\"sdk_fixture.jl\")\n\
             using .LulangSdkFixture\n\
             input = Float64[1, 2, 3]\n\
             @assert LulangSdkFixture.borrowed_sum(input) == 6\n\
             values = Float64[1.5, 2.5]\n\
             @assert LulangSdkFixture.borrowed_bump(values) == 2.5\n\
             @assert values == Float64[2.5, 3.5]\n\
             @assert LulangSdkFixture.vec2_sum(LulangSdkFixture.Vec2(2, 5)) == 7\n\
             made = LulangSdkFixture.make_vec2(1.25, 3.75)\n\
             @assert made.x == 1.25 && made.y == 3.75\n\
             owned = LulangSdkFixture.make_values(4)\n\
             @assert LulangSdkFixture.borrow(owned) == Float64[0, 0.5, 1, 1.5]\n\
             close(owned)\n\
             @assert LulangSdkFixture.positive(3)\n\
             @assert LulangSdkFixture.checked_div(12, 3) == 4\n\
             increment(value::Int64)::Int64 = value + 1\n\
             callback = @cfunction(increment, Int64, (Int64,))\n\
             @assert ccall(LulangSdkFixture.callback_identity(callback), Int64, (Int64,), 41) == 42\n\
             try\n\
               LulangSdkFixture.checked_div(12, 0)\n\
               error(\"expected LulangError\")\n\
             catch failure\n\
               @assert failure isa LulangSdkFixture.LulangError && failure.code == 1\n\
             end\n\
             @assert LulangSdkFixture.greeting(UInt8[0x41]) == UInt8[0x41, 0x00, 0x21]\n\
             println(\"julia sdk ok\")\n",
        )
        .expect("write Julia harness");
        let output = run(Command::new("julia")
            .current_dir(&directory)
            .env("LULANG_LIBRARY", shared_library(&directory))
            .arg("--startup-file=no")
            .arg(&julia_harness));
        assert_eq!(String::from_utf8_lossy(&output.stdout), "julia sdk ok\n");
    }

    let go_source =
        std::fs::read_to_string(go_sdk.join("sdk_fixture.go")).expect("read generated Go SDK");
    assert!(go_source.contains("func BorrowedSum"));
    assert!(go_source.contains("type OwnedF64"));
    std::fs::write(
        go_sdk.join("sdk_fixture_test.go"),
        "package sdk_fixture\n\
         import \"testing\"\n\
         func TestGeneratedSDK(t *testing.T) {\n\
           input := []float64{1, 2, 3}\n\
           if BorrowedSum(input) != 6 { t.Fatal(\"sum\") }\n\
           values := []float64{1.5, 2.5}\n\
           if BorrowedBump(values) != 2.5 || values[0] != 2.5 { t.Fatal(\"bump\") }\n\
           if Vec2Sum(Vec2{X: 2, Y: 5}) != 7 { t.Fatal(\"record\") }\n\
           made := MakeVec2(1.25, 3.75)\n\
           if made.X != 1.25 || made.Y != 3.75 { t.Fatal(\"record return\") }\n\
           owned := MakeValues(4)\n\
           if len(owned.Values()) != 4 || owned.Values()[3] != 1.5 { t.Fatal(\"owned\") }\n\
           if err := owned.Close(); err != nil { t.Fatal(err) }\n\
           if !Positive(3) { t.Fatal(\"bool\") }\n\
           if value, err := CheckedDiv(12, 3); err != nil || value != 4 { t.Fatal(\"result\") }\n\
           if _, err := CheckedDiv(12, 0); err == nil { t.Fatal(\"error\") }\n\
           greeting := Greeting([]byte{'A'})\n\
           if len(greeting) != 3 || greeting[1] != 0 { t.Fatal(\"string\") }\n\
         }\n",
    )
    .expect("write Go SDK test");
    if Command::new("go").arg("version").output().is_ok() {
        run(Command::new("go")
            .arg("test")
            .arg("./...")
            .current_dir(&go_sdk)
            .env("CGO_ENABLED", "1"));
    }

    run(Command::new("node")
        .args(["--check", "index.js"])
        .current_dir(&node_sdk));
    if Command::new("swift").arg("--version").output().is_ok() {
        run(Command::new("swift").arg("build").current_dir(&swift_sdk));
        let swift_harness = directory.join("swift-harness");
        std::fs::create_dir_all(swift_harness.join("Sources/SDKHarness"))
            .expect("create Swift harness");
        std::fs::write(
            swift_harness.join("Package.swift"),
            "// swift-tools-version: 5.9\n\
             import PackageDescription\n\
             let package = Package(name: \"SDKHarness\", dependencies: [.package(path: \"../sdk_fixture-swift\")], targets: [.executableTarget(name: \"SDKHarness\", dependencies: [.product(name: \"LulangSdkFixture\", package: \"sdk_fixture-swift\")])])\n",
        )
        .expect("write Swift harness package");
        std::fs::write(
            swift_harness.join("Sources/SDKHarness/main.swift"),
            "import LulangSdkFixture\n\
             let input = [1.0, 2.0, 3.0]\n\
             precondition(BorrowedSum(input) == 6)\n\
             var values = [1.5, 2.5]\n\
             precondition(BorrowedBump(&values) == 2.5 && values[0] == 2.5)\n\
             precondition(Vec2Sum(Vec2(x: 2, y: 5)) == 7)\n\
             let made = MakeVec2(1.25, 3.75)\n\
             precondition(made.x == 1.25 && made.y == 3.75)\n\
             let owned = MakeValues(4)\n\
             precondition(owned.values.count == 4 && owned.values[3] == 1.5)\n\
             owned.close()\n\
             precondition(Positive(3))\n\
             let divided = try CheckedDiv(12, 3)\n\
             precondition(divided == 4)\n\
             func increment(_ value: Int64) -> Int64 { value + 1 }\n\
             let callback = CallbackIdentity(increment)\n\
             precondition(callback(41) == 42)\n\
             do { _ = try CheckedDiv(12, 0); preconditionFailure(\"missing error\") } catch let error as LulangError { precondition(error.code == 1) }\n\
             let bytes = Greeting([65])\n\
             precondition(bytes == [65, 0, 33])\n\
             print(\"swift sdk ok\")\n",
        )
        .expect("write Swift harness");
        let output = run(Command::new("swift").arg("run").current_dir(&swift_harness));
        assert_eq!(String::from_utf8_lossy(&output.stdout), "swift sdk ok\n");
    }
    assert!(r_sdk.join("DESCRIPTION").is_file());
    if Command::new("Rscript").arg("--version").output().is_ok() {
        let r_library = directory.join("r-library");
        std::fs::create_dir_all(&r_library).expect("create R library");
        run(Command::new("R")
            .args(["CMD", "INSTALL", "-l"])
            .arg(&r_library)
            .arg(&r_sdk));
        let r_harness = directory.join("r_harness.R");
        std::fs::write(
            &r_harness,
            format!(
                "library(LulangSdkFixture, lib.loc={:?})\n\
                 stopifnot(borrowed_sum(c(1, 2, 3)) == 6)\n\
                 values <- c(1.5, 2.5)\n\
                 stopifnot(borrowed_bump(values) == 2.5)\n\
                 stopifnot(vec2_sum(list(2, 5)) == 7)\n\
                 made <- make_vec2(1.25, 3.75)\n\
                 stopifnot(made$x == 1.25, made$y == 3.75)\n\
                 stopifnot(identical(as.numeric(make_values(4)), c(0, 0.5, 1, 1.5)))\n\
                 stopifnot(positive(3), checked_div(12, 3) == 4)\n\
                 failed <- tryCatch({{ checked_div(12, 0); FALSE }}, error=function(e) TRUE)\n\
                 stopifnot(failed, identical(as.integer(greeting(as.raw(65))), c(65L, 0L, 33L)))\n\
                 cat('r sdk ok\\n')\n",
                r_library
            ),
        )
        .expect("write R harness");
        let output = run(Command::new("Rscript").arg("--vanilla").arg(&r_harness));
        assert_eq!(String::from_utf8_lossy(&output.stdout), "r sdk ok\n");
    }

    let _ = std::fs::remove_dir_all(directory);
}

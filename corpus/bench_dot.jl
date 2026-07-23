# Julia twin of bench_dot.lu — dense Float64 vectors and the standard dot loop.

function dot_kernel(a, b)
    total = 0.0
    @inbounds @simd for i in eachindex(a, b)
        total += a[i] * b[i]
    end
    total
end

n = 2_000_000
a = Float64.(0:n-1) .* 0.000001
b = Float64.(0:n-1) .* 0.000002
acc = 0.0
for _ in 1:20
    global acc += dot_kernel(a, b)
end
println("acc: ", acc)

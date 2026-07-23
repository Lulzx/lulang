# Julia reference formulas for every public lu-numerics kernel.
using LinearAlgebra
dot(x, y, n) = LinearAlgebra.dot(x[1:n], y[1:n])
norm2(x, n) = norm(x[1:n])
axpy(a, x, y, n) = (y[1:n] .= a .* x[1:n] .+ y[1:n])
scale(a, x, n) = (x[1:n] .*= a)
mean(x, n) = sum(x[1:n]) / n
variance(x, n) = sum((x[1:n] .- mean(x, n)).^2) / n
mse(x, y, n) = sum((x[1:n] .- y[1:n]).^2) / n
trapz_uniform(y, dx, n) = n < 2 ? 0.0 : dx * (0.5y[1] + sum(y[2:n-1]) + 0.5y[n])
trapz_xy(x, y, n) = n < 2 ? 0.0 : sum(0.5 .* diff(x[1:n]) .* (y[1:n-1] .+ y[2:n]))
polynomial_eval(c, x, n) = foldr((coefficient, result) -> result*x+coefficient, c[1:n]; init=0.0)
gemv_inplace(matrix, x, rows, columns) = (x[1:rows] .= reshape(matrix, columns, rows)' * copy(x[1:columns]))
matmul_square_inplace(a, b, n) = (a .= vec(reshape(copy(a), n, n) * reshape(b, n, n)))
convolution_inplace(signal, kernel, output_n, kernel_n) = (input=copy(signal); signal[1:output_n] .= [sum(input[i-j+1]*kernel[j] for j=1:min(i,kernel_n)) for i=1:output_n])
moving_average_inplace(signal, window, n) = (input=copy(signal); signal[1:n] .= [sum(input[max(1,i-window+1):i])/min(i,window) for i=1:n])
lcg_step(seed) = rem(abs(rem(seed, 2147483647)) * 48271 + 1, 2147483647)
function monte_carlo_pi(samples, seed)
    inside = 0
    for _=1:samples
        seed=lcg_step(seed); x=rem(seed,1000000)/1000000
        seed=lcg_step(seed); y=rem(seed,1000000)/1000000
        inside += x*x+y*y <= 1
    end
    samples > 0 ? 4inside/samples : 0.0
end
function bisect_sqrt(value, iterations)
    value <= 0 && return 0.0
    low, high = 0.0, max(1.0,value)
    for _=1:iterations
        middle=(low+high)/2
        if middle*middle > value; high=middle else low=middle end
    end
    (low+high)/2
end
sinc(x) = x == 0 ? 1.0 : sin(x)/x
normal_pdf(x) = 0.3989422804014327 * 2.718281828459045^(-0.5x*x)
sigmoid(x) = 1/(1+2.718281828459045^(-x))
clamp(x, low, high) = min(high,max(low,x))
lerp(a,b,t) = a+(b-a)*t
distance3(ax,ay,az,bx,by,bz) = sqrt((bx-ax)^2+(by-ay)^2+(bz-az)^2)
determinant2(a,b,c,d) = a*d-b*c
factorial(n) = prod(2:n; init=1)
binomial(n,k) = 0 <= k <= n ? Base.binomial(n,k) : 0
